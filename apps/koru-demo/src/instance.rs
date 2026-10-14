// SPDX-License-Identifier: MPL-2.0

//! **Which Koru, and on whose storage — decided before anything is inspected.**
//!
//! §7.5 of the roadmap: *"Runner setup must identify the selected root and
//! instance; no guessing a workspace, attaching to the user's ordinary window,
//! or clearing a pre-existing root."* That is three separate refusals, and this
//! module is where each one is made:
//!
//! - **No guessing a workspace.** `--root` is required. There is deliberately no
//!   default, no search path and no environment fall-back — the same rule
//!   `koru-voice-bench` applies to its corpus, for the same reason: a runner
//!   that silently found *some* root would be reporting about a workspace
//!   nobody named.
//! - **No attaching to the user's ordinary window.** The runner never
//!   enumerates windows looking for something that looks like Koru. It is given
//!   a window handle, and it then proves three things about what answered: the
//!   process is the one named, the class is Koru's window class, and the root
//!   published a `koru:` locator. An ordinary session fails the first of those.
//! - **No clearing a pre-existing root.** This crate writes nothing, anywhere.
//!   There is no cleanup to restrict, which is the cheapest way to satisfy a
//!   rule about cleanup.
//!
//! ## A live rendezvous is what makes a root more than a path
//!
//! A directory that exists is not an instance. `koru_api_discovery::probe_at`
//! asks the root's own gateway rendezvous whether anything is serving there —
//! it is the question `koru-agent status` already asks, and it mints no
//! credential, so a liveness check cannot leave a session behind. A root that
//! answers is a rooted Koru; a root that does not is a directory, and the
//! difference is the whole of *isolated* in the M3 exit.
//!
//! **What it does not establish** is that the window handle belongs to *that*
//! instance. The gateway is the Agent's worker and the window is `koru.exe`;
//! they are different processes with different pids. So the two checks are
//! reported separately and neither is inferred from the other — see
//! [`Identified`].

use std::path::{Path, PathBuf};

use crate::locator;

/// Koru's own window class, as `accesskit_windows` reports it from
/// `Tree::toolkit_name`'s sibling `Node::class_name`.
///
/// Recorded in `guide/contributing/deps/ACCESSKIT.md`'s property table. A window that answers
/// something else is not one of ours, whatever its title says — and a title is
/// the one thing this runner may not look at anyway.
pub const WINDOW_CLASS: &str = "KoruWindow";

/// The framework id every Koru tree carries, from `Tree::toolkit_name`.
pub const FRAMEWORK: &str = "Koru";

/// What the runner was pointed at, before it has looked at anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub root: PathBuf,
    pub process_id: u32,
    pub window_handle: i64,
}

/// What a window turned out to be.
///
/// Every field is something read off the root element rather than assumed, and
/// the two liveness facts are kept apart on purpose: the gateway serving a root
/// and this window belonging to that root's Koru are different claims, and the
/// second one is not established here. See the module note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identified {
    pub root: PathBuf,
    /// Whether the named root's own rendezvous is serving.
    pub root_is_serving: bool,
    pub process_id: u32,
    pub window_handle: i64,
    /// The window slot the root's own locator carries. Every locator the run
    /// then builds is scoped by this rather than by a guessed `w1`.
    pub window_slot: u64,
    pub class_name: Option<String>,
    pub framework: Option<String>,
    /// **The binary this window is actually running**, and when it was built.
    ///
    /// Not a nicety. A running Koru holds `target\debug\koru.exe` open, so
    /// every `cargo build` after it started fails to replace that file and
    /// silently leaves the old one in place — the process goes on running code
    /// from before whatever you just merged. From outside, a window whose view
    /// predates a projection is indistinguishable from a window whose view is
    /// broken: both answer *no control has that locator*. Measured on
    /// 2026-09-11, when exactly that cost an investigation.
    ///
    /// `None` when the path could not be read, which is not a refusal: the
    /// runner still runs, it just cannot tell you this.
    pub binary: Option<Binary>,
}

/// The executable a process is running, and its mtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binary {
    pub path: std::path::PathBuf,
    /// Formatted rather than a `SystemTime`, because the only thing anybody
    /// does with it is compare it by eye against when they last built.
    pub built: Option<String>,
}

impl std::fmt::Display for Identified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "root:     {}", self.root.display())?;
        writeln!(
            f,
            "serving:  {}",
            if self.root_is_serving {
                "yes"
            } else {
                "no — nothing is answering this root's rendezvous"
            }
        )?;
        writeln!(f, "process:  {}", self.process_id)?;
        writeln!(
            f,
            "window:   slot {} (handle {})",
            self.window_slot, self.window_handle
        )?;
        writeln!(
            f,
            "class:    {} / framework {}",
            self.class_name.as_deref().unwrap_or("<none>"),
            self.framework.as_deref().unwrap_or("<none>")
        )?;
        match &self.binary {
            Some(binary) => write!(
                f,
                "binary:   {}{}",
                binary.path.display(),
                match &binary.built {
                    Some(built) => format!("  (built {built})"),
                    None => String::new(),
                }
            ),
            None => write!(f, "binary:   <could not read this process's image path>"),
        }
    }
}

/// Why the runner will not run against what it was pointed at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotIdentified {
    /// The named root is not a directory.
    NoSuchRoot(PathBuf),
    /// The window's root element published no Koru locator, so this is not a
    /// Koru window — or is one whose accessibility tree was never installed,
    /// which is the same thing from out here.
    NotKoru { automation_id: String },
    /// The window is in a different process from the one named.
    OtherProcess { named: u32, answered: Option<u32> },
    /// It is a Koru window, and it belongs to a *second* Koru.
    WrongClass { class_name: Option<String> },
}

impl std::fmt::Display for NotIdentified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchRoot(path) => write!(
                f,
                "--root {} is not a directory. There is deliberately no default root and no \
                 search path: a run that found some workspace by itself would be reporting \
                 about one nobody named",
                path.display()
            ),
            Self::NotKoru { automation_id } => write!(
                f,
                "that window's root element answers `{automation_id}` rather than a `{}` \
                 locator, so it is not a Koru window with an accessibility tree installed",
                locator::PREFIX
            ),
            Self::OtherProcess { named, answered } => write!(
                f,
                "that window belongs to process {}, and --pid named {named}",
                match answered {
                    Some(pid) => pid.to_string(),
                    None => "an unknown process".to_owned(),
                }
            ),
            Self::WrongClass { class_name } => write!(
                f,
                "that window's class is {} rather than {WINDOW_CLASS}",
                class_name.as_deref().unwrap_or("<none>")
            ),
        }
    }
}

/// What the runner read off the window it was pointed at.
///
/// Plain data, so the rule below is a pure function and the platform half has
/// nothing to decide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootElement {
    pub automation_id: String,
    pub class_name: Option<String>,
    pub framework: Option<String>,
    pub process_id: Option<u32>,
}

/// Apply the rule.
///
/// `root_exists` and `root_is_serving` are passed in rather than looked up, so
/// this stays a pure function with no disk and no IPC — the two things a test
/// must not do.
pub fn identify(
    target: &Target,
    root_exists: bool,
    root_is_serving: bool,
    element: &RootElement,
) -> Result<Identified, NotIdentified> {
    if !root_exists {
        return Err(NotIdentified::NoSuchRoot(target.root.clone()));
    }
    // Process before anything else. Every other check is a question about a
    // window, and asking questions about the wrong process's window is the
    // failure §7.5 names.
    if element.process_id != Some(target.process_id) {
        return Err(NotIdentified::OtherProcess {
            named: target.process_id,
            answered: element.process_id,
        });
    }
    if element.class_name.as_deref() != Some(WINDOW_CLASS) {
        return Err(NotIdentified::WrongClass {
            class_name: element.class_name.clone(),
        });
    }
    let window_slot =
        locator::window_slot_of(&element.automation_id).ok_or_else(|| NotIdentified::NotKoru {
            automation_id: element.automation_id.clone(),
        })?;

    Ok(Identified {
        root: target.root.clone(),
        root_is_serving,
        process_id: target.process_id,
        window_handle: target.window_handle,
        window_slot,
        class_name: element.class_name.clone(),
        framework: element.framework.clone(),
        binary: binary_of(target.process_id),
    })
}

/// The image a process is running, read from the process rather than guessed
/// from the window. See [`Identified::binary`] for why this is here at all.
#[cfg(windows)]
pub fn binary_of(process_id: u32) -> Option<Binary> {
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    // SAFETY: a query-only handle, closed on every path below, and the buffer
    // is sized before the call and read only up to what the call reported.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut buffer = [0u16; MAX_PATH as usize];
        let mut len = buffer.len() as u32;
        let read = QueryFullProcessImageNameW(
            handle,
            Default::default(),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        read.ok()?;
        let path = std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
        let built = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(format_built);
        Some(Binary { path, built })
    }
}

#[cfg(not(windows))]
pub fn binary_of(_process_id: u32) -> Option<Binary> {
    None
}

/// **How long ago**, not when. The question a person actually has is *is this
/// the code I just built*, and an age answers it without them doing arithmetic
/// against a timestamp in some timezone.
// Only the Windows `binary_of` and the tests format a build time.
#[cfg(any(windows, test))]
fn format_built(at: std::time::SystemTime) -> String {
    let Ok(age) = std::time::SystemTime::now().duration_since(at) else {
        // Built in the future: a clock moved, and saying so is more use than a
        // negative number.
        return "at a time later than now — check the clock".to_string();
    };
    let mins = age.as_secs() / 60;
    match mins {
        0 => "under a minute ago".to_string(),
        1..=90 => format!("{mins} min ago"),
        _ => format!("{} h {} min ago", mins / 60, mins % 60),
    }
}

/// The root as the **rendezvous** spells it.
///
/// **A relative `--root` is not the same root to `koru-ipc`.** A rooted
/// endpoint's name is a digest of `root_identity`, which is *lexical*: it
/// lowercases and flips separators and deliberately does not touch the disk, so
/// `.dev-data\root-mode` and `D:\…\.dev-data\root-mode` hash to two different
/// pipes. The Agent published under the absolute one, and a probe with the
/// relative one found nothing there.
///
/// That produced a **false negative that reads like a real failure**: *nothing
/// is answering this root's rendezvous*, printed at somebody whose Store was
/// fine and who had simply typed a path relative to their shell. Measured
/// 2026-09-11 — the same instance answered `no` relative and `yes` absolute.
///
/// [`std::path::absolute`] rather than [`std::fs::canonicalize`], to match: it
/// is lexical too, it opens nothing, and it does not resolve symlinks into a
/// `\\?\` form whose spelling is a third thing again.
pub fn resolved(root: &Path) -> PathBuf {
    std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Ask the named root's own rendezvous whether anything is serving there.
///
/// The liveness question `koru-agent status` asks, on the channel this build
/// belongs to. It mints no credential: a check that left a session behind would
/// be a runner changing the thing it was sent to observe.
///
/// **Pass [`resolved`]'s answer**, not what a person typed.
pub fn root_is_serving(root: &Path) -> bool {
    koru_api_discovery::probe_at(koru_channel::segment(), root).is_ok()
}

#[cfg(test)]
mod tests;
