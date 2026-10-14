// SPDX-License-Identifier: MPL-2.0

//! Private lifecycle and activation client for the standalone AI chat host.
//!
//! Every dial and wait in this module is blocking. Event-loop callers must use
//! [`open_off_thread`] or [`probe_off_thread`]; startup and update workers may
//! call the synchronous primitives directly.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use koru_agent_wire::{
    AI_CHAT_PROTOCOL_VERSION, AiChatRequest, AiChatResponse, FromAgent, Response,
};

/// Where an asynchronous open reports its completion.
///
/// A workspace owns a correlated request and receives the ordinary wire
/// response. Agent-owned gestures have no requester, so their result returns
/// through the event mailbox to the only thread allowed to touch its overlay.
#[derive(Clone)]
pub enum OpenCompletion {
    Shell { sender: Sender<FromAgent>, id: u64 },
    Agent(crate::event::Waker),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub focused: bool,
}

#[derive(Clone, Debug)]
pub enum Open {
    Landing,
    Chat {
        chat: uuid::Uuid,
        block: Option<uuid::Uuid>,
    },
}

impl Open {
    fn request(&self) -> AiChatRequest {
        match self {
            Self::Landing => AiChatRequest::Activate,
            Self::Chat { chat, block } => AiChatRequest::OpenChat {
                chat: chat.to_string(),
                block: block.map(|block| block.to_string()),
            },
        }
    }

    fn link(&self) -> Option<String> {
        match self {
            Self::Landing => None,
            Self::Chat { chat, block } => Some(koru_link::chat_link(*chat, *block)),
        }
    }
}

fn endpoint(root: Option<&Path>) -> Result<koru_ipc::Endpoint, koru_ipc::IpcError> {
    match root {
        Some(root) => {
            koru_ipc::Endpoint::for_service_root(koru_ipc::AI_CHAT, koru_channel::segment(), root)
        }
        None => koru_ipc::Endpoint::for_service(koru_ipc::AI_CHAT, koru_channel::segment()),
    }
}

fn request(root: Option<&Path>, body: &AiChatRequest, timeout: Duration) -> Option<AiChatResponse> {
    let endpoint = endpoint(root).ok()?;
    let mut stream = koru_ipc::connect_timed(&endpoint, timeout).ok()?;
    koru_ipc::frame::write_frame(&mut stream, AI_CHAT_PROTOCOL_VERSION, body).ok()?;
    let (version, response) = koru_ipc::frame::read_frame::<_, AiChatResponse>(&mut stream).ok()?;
    (version == AI_CHAT_PROTOCOL_VERSION).then_some(response)
}

pub fn status(root: Option<&Path>, timeout: Duration) -> Option<Process> {
    match request(root, &AiChatRequest::Status, timeout)? {
        AiChatResponse::Accepted { pid, focused } => Some(Process { pid, focused }),
        AiChatResponse::Failed { .. } => None,
    }
}

pub fn open(root: Option<&Path>, open: &Open) -> std::io::Result<u32> {
    if let Some(process) = status(root, Duration::from_secs(1)) {
        // Permission and direct activation must precede the request: receiving
        // it wakes the Chat loop, which focuses its own HWND immediately.
        if process.pid != 0 {
            let _ = crate::app::foreground(process.pid);
        }
        return match request(root, &open.request(), Duration::from_secs(1)) {
            Some(AiChatResponse::Accepted { pid, .. }) => Ok(pid),
            Some(AiChatResponse::Failed { message, .. }) => Err(std::io::Error::other(message)),
            None => Err(std::io::Error::other(
                "the running Koru AI chat did not accept activation",
            )),
        };
    }
    launch(root, open.link().as_deref())
}

pub fn launch(root: Option<&Path>, link: Option<&str>) -> std::io::Result<u32> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| std::io::Error::other("this executable has no directory"))?;
    let candidate = dir.join(koru_update::AI_CHAT_EXE);
    if !candidate.exists() {
        return Err(std::io::Error::other(format!(
            "{} is not beside this executable",
            candidate.display()
        )));
    }
    let mut command = launch_command(&candidate, root, link);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::app::detach(&mut command);
    let pid = crate::app::remember_launch(command.spawn()?);
    // The process may not have created an HWND yet, but the foreground grant
    // is useful immediately and the direct attempt is harmless. The host also
    // self-activates when it drains its initial link.
    let _ = crate::app::foreground(pid);
    Ok(pid)
}

fn launch_command(
    candidate: &Path,
    root: Option<&Path>,
    link: Option<&str>,
) -> std::process::Command {
    let mut command = std::process::Command::new(candidate);
    if let Some(link) = link {
        command.args(["--chat-link", link]);
    }
    if let Some(root) = root {
        // `Path` is an `OsStr`, not text. Converting through `display()` is
        // lossy for valid Windows paths containing unpaired UTF-16 and can
        // silently point Chat at a different data root and singleton endpoint.
        command.arg("--root").arg(root);
    }
    command
}

pub fn open_off_thread(root: Option<PathBuf>, open: Open, completion: OpenCompletion) {
    start_open_worker(root, open, completion, |job| {
        std::thread::Builder::new()
            .name("koru-agent AI chat open".into())
            .spawn(job)
            .map(|_| ())
    });
}

fn start_open_worker(
    root: Option<PathBuf>,
    open: Open,
    completion: OpenCompletion,
    start: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) {
    // `spawn` consumes the closure even when the OS refuses the thread, so the
    // fallback needs its own completion handle. Otherwise the rarest failure
    // would be the one path that neither answers the shell nor wakes Agent.
    let spawn_failure = completion.clone();
    let job = Box::new(move || {
        finish_open(completion, self::open(root.as_deref(), &open));
    });
    if let Err(error) = start(job) {
        finish_open(
            spawn_failure,
            Err(std::io::Error::other(format!(
                "the Koru AI chat open worker could not start: {error}"
            ))),
        );
    }
}

fn finish_open(completion: OpenCompletion, result: std::io::Result<u32>) {
    match completion {
        OpenCompletion::Shell { sender, id } => {
            let body = match result {
                Ok(_) => Response::Ok,
                Err(error) => Response::Failed {
                    code: "launch_failed".into(),
                    message: error.to_string(),
                },
            };
            let _ = sender.send(FromAgent::Response { id, body });
        }
        OpenCompletion::Agent(waker) => {
            let error = result.err().map(|error| {
                tracing::error!(%error, "Koru AI chat could not be opened");
                error.to_string()
            });
            waker.wake(crate::event::AgentEvent::AiChatOpened { error });
        }
    }
}

pub fn probe_off_thread(root: Option<PathBuf>, waker: crate::event::Waker) {
    start_probe_worker(root, waker, |job| {
        std::thread::Builder::new()
            .name("koru-agent AI chat status".into())
            .spawn(job)
            .map(|_| ())
    });
}

fn start_probe_worker(
    root: Option<PathBuf>,
    waker: crate::event::Waker,
    start: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) {
    let spawn_failure = waker.clone();
    let job = Box::new(move || {
        let process = status(root.as_deref(), Duration::from_secs(1));
        waker.wake(crate::event::AgentEvent::AiChatProbed { process });
    });
    if let Err(error) = start(job) {
        let error = format!("the Koru AI chat status worker could not start: {error}");
        tracing::error!(%error, "Koru AI chat could not be checked");
        spawn_failure.wake(crate::event::AgentEvent::AiChatOpened { error: Some(error) });
    }
}

/// Ask a known-live singleton to exit and wait until its endpoint disappears.
/// `before_exit` records restart intent after liveness is proven and before the
/// request can close the process.
pub fn stop_for_update(
    root: Option<&Path>,
    timeout: Duration,
    before_exit: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    if status(root, Duration::from_secs(1)).is_none() {
        return Ok(false);
    }
    request_exit_after(before_exit, || {
        request(root, &AiChatRequest::Exit, Duration::from_secs(1))
    })?;
    let started = Instant::now();
    while started.elapsed() < timeout {
        if status(root, Duration::from_millis(200)).is_none() {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(format!(
        "Koru AI chat did not close within {} seconds",
        timeout.as_secs()
    ))
}

/// Persist restart intent before the exit request crosses the process boundary.
/// If persistence fails, the live chat is left alone so an aborted update can
/// never turn a bookkeeping failure into a silently lost window.
fn request_exit_after(
    before_exit: impl FnOnce() -> Result<(), String>,
    request_exit: impl FnOnce() -> Option<AiChatResponse>,
) -> Result<(), String> {
    before_exit()?;
    match request_exit() {
        Some(AiChatResponse::Accepted { .. }) => {}
        Some(AiChatResponse::Failed { message, .. }) => return Err(message),
        None => return Err("Koru AI chat did not acknowledge the exit request".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn open_request_preserves_exact_chat_and_block() {
        let chat = uuid::Uuid::new_v4();
        let block = uuid::Uuid::new_v4();
        assert_eq!(
            Open::Chat {
                chat,
                block: Some(block)
            }
            .request(),
            AiChatRequest::OpenChat {
                chat: chat.to_string(),
                block: Some(block.to_string())
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn rooted_launch_preserves_non_unicode_os_path_bytes() {
        use std::os::windows::ffi::OsStringExt as _;

        let component = std::ffi::OsString::from_wide(&[b'r' as u16, 0xd800, b't' as u16]);
        let root = PathBuf::from("C:/").join(component);
        let command = launch_command(Path::new("koru-ai-chat.exe"), Some(&root), None);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, [std::ffi::OsStr::new("--root"), root.as_os_str()]);
    }

    #[test]
    fn a_worker_spawn_failure_answers_a_waiting_shell() {
        let (sender, receiver) = mpsc::channel();
        start_open_worker(
            None,
            Open::Landing,
            OpenCompletion::Shell { sender, id: 41 },
            |_job| Err(std::io::Error::other("thread limit")),
        );

        assert!(matches!(
            receiver.recv(),
            Ok(FromAgent::Response {
                id: 41,
                body: Response::Failed { code, message }
            }) if code == "launch_failed"
                && message.contains("open worker could not start")
                && message.contains("thread limit")
        ));
    }

    #[test]
    fn a_worker_spawn_failure_wakes_the_agent_surface() {
        let (sender, receiver) = mpsc::channel();
        let waker = crate::event::Waker::new(move |event| {
            let _ = sender.send(event);
        });
        start_open_worker(None, Open::Landing, OpenCompletion::Agent(waker), |_job| {
            Err(std::io::Error::other("thread limit"))
        });

        assert!(matches!(
            receiver.recv(),
            Ok(crate::event::AgentEvent::AiChatOpened { error: Some(error) })
                if error.contains("open worker could not start")
                    && error.contains("thread limit")
        ));
    }

    #[test]
    fn a_probe_worker_spawn_failure_wakes_the_agent_surface() {
        let (sender, receiver) = mpsc::channel();
        let waker = crate::event::Waker::new(move |event| {
            let _ = sender.send(event);
        });
        start_probe_worker(None, waker, |_job| {
            Err(std::io::Error::other("thread limit"))
        });

        assert!(matches!(
            receiver.recv(),
            Ok(crate::event::AgentEvent::AiChatOpened { error: Some(error) })
                if error.contains("status worker could not start")
                    && error.contains("thread limit")
        ));
    }

    #[test]
    fn a_reopen_persistence_failure_prevents_the_exit_request() {
        let exit_requested = std::cell::Cell::new(false);

        let result = request_exit_after(
            || Err("disk full".into()),
            || {
                exit_requested.set(true);
                Some(AiChatResponse::Accepted {
                    pid: std::process::id(),
                    focused: false,
                })
            },
        );

        assert_eq!(result.unwrap_err(), "disk full");
        assert!(!exit_requested.get());
    }
}
