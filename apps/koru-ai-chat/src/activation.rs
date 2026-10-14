// SPDX-License-Identifier: MPL-2.0

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use koru_agent_wire::{AI_CHAT_PROTOCOL_VERSION, AiChatRequest, AiChatResponse};

pub type ChatLink = (koru_ai_core::ChatId, Option<koru_ai_core::MessageBlockId>);

#[derive(Clone)]
pub struct Activation {
    focus: Arc<AtomicBool>,
    focused: Arc<AtomicBool>,
    exit: Arc<AtomicBool>,
    links: Arc<Mutex<Vec<ChatLink>>>,
    pub wake: Arc<Mutex<Option<winit::event_loop::EventLoopProxy<()>>>>,
}

impl Activation {
    pub fn take_focus(&self) -> bool {
        self.focus.swap(false, Ordering::AcqRel)
    }

    pub fn take_exit(&self) -> bool {
        self.exit.swap(false, Ordering::AcqRel)
    }

    pub fn take_links(&self) -> Vec<ChatLink> {
        self.links
            .lock()
            .map(|mut links| std::mem::take(&mut *links))
            .unwrap_or_default()
    }

    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Release);
    }

    fn wake(&self) {
        if let Ok(wake) = self.wake.lock()
            && let Some(proxy) = wake.as_ref()
        {
            let _ = proxy.send_event(());
        }
    }
}

pub fn endpoint(root: Option<&Path>) -> Result<koru_ipc::Endpoint, koru_ipc::IpcError> {
    match root {
        Some(root) => {
            koru_ipc::Endpoint::for_service_root(koru_ipc::AI_CHAT, koru_channel::segment(), root)
        }
        None => koru_ipc::Endpoint::for_service(koru_ipc::AI_CHAT, koru_channel::segment()),
    }
}

pub fn bind(root: Option<&Path>) -> Option<Activation> {
    let listener = koru_ipc::Listener::bind(endpoint(root).ok()?).ok()?;
    let activation = Activation {
        focus: Arc::new(AtomicBool::new(false)),
        focused: Arc::new(AtomicBool::new(false)),
        exit: Arc::new(AtomicBool::new(false)),
        links: Arc::new(Mutex::new(Vec::new())),
        wake: Arc::new(Mutex::new(None)),
    };
    let thread_activation = activation.clone();
    std::thread::Builder::new()
        .name("koru-ai-chat-activation".into())
        .spawn(move || {
            loop {
                let Ok(mut stream) = listener.accept() else {
                    continue;
                };
                let Ok((version, request)) =
                    koru_ipc::frame::read_frame::<_, AiChatRequest>(&mut stream)
                else {
                    continue;
                };
                if version != AI_CHAT_PROTOCOL_VERSION {
                    continue;
                }
                let (response, wake) = handle(&thread_activation, request);
                let _ =
                    koru_ipc::frame::write_frame(&mut stream, AI_CHAT_PROTOCOL_VERSION, &response);
                // Exit is acknowledged before the UI thread can tear down the
                // process and close this stream.
                if wake {
                    thread_activation.wake();
                }
            }
        })
        .ok()?;
    Some(activation)
}

fn handle(activation: &Activation, request: AiChatRequest) -> (AiChatResponse, bool) {
    let result = match request {
        AiChatRequest::Activate => {
            activation.focus.store(true, Ordering::Release);
            Ok(true)
        }
        AiChatRequest::OpenChat { chat, block } => {
            let parsed = uuid::Uuid::parse_str(&chat)
                .ok()
                .zip(block.as_deref().map(uuid::Uuid::parse_str).transpose().ok());
            match parsed {
                Some((chat, block)) => {
                    if let Ok(mut links) = activation.links.lock() {
                        links.push((
                            koru_ai_core::ChatId::from_uuid(chat),
                            block.map(koru_ai_core::MessageBlockId::from_uuid),
                        ));
                    }
                    activation.focus.store(true, Ordering::Release);
                    Ok(true)
                }
                None => Err(("bad_link", "a chat link whose ids are not uuids")),
            }
        }
        AiChatRequest::Status => Ok(false),
        AiChatRequest::Exit => {
            activation.exit.store(true, Ordering::Release);
            Ok(true)
        }
    };
    match result {
        Ok(wake) => (
            AiChatResponse::Accepted {
                pid: std::process::id(),
                focused: activation.focused.load(Ordering::Acquire),
            },
            wake,
        ),
        Err((code, message)) => (
            AiChatResponse::Failed {
                code: code.into(),
                message: message.into(),
            },
            false,
        ),
    }
}

pub fn request(
    root: Option<&Path>,
    request: &AiChatRequest,
    timeout: Duration,
) -> Option<AiChatResponse> {
    let endpoint = endpoint(root).ok()?;
    let mut stream = koru_ipc::connect_timed(&endpoint, timeout).ok()?;
    koru_ipc::frame::write_frame(&mut stream, AI_CHAT_PROTOCOL_VERSION, request).ok()?;
    let (version, response) = koru_ipc::frame::read_frame::<_, AiChatResponse>(&mut stream).ok()?;
    (version == AI_CHAT_PROTOCOL_VERSION).then_some(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activation(focused: bool) -> Activation {
        Activation {
            focus: Arc::new(AtomicBool::new(false)),
            focused: Arc::new(AtomicBool::new(focused)),
            exit: Arc::new(AtomicBool::new(false)),
            links: Arc::new(Mutex::new(Vec::new())),
            wake: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn exact_link_and_exit_are_queued_for_the_shell() {
        let activation = activation(true);
        let chat = uuid::Uuid::new_v4();
        let block = uuid::Uuid::new_v4();
        assert!(matches!(
            handle(
                &activation,
                AiChatRequest::OpenChat {
                    chat: chat.to_string(),
                    block: Some(block.to_string()),
                }
            )
            .0,
            AiChatResponse::Accepted { focused: true, .. }
        ));
        let links = activation.take_links();
        assert_eq!(links[0].0.as_uuid(), chat);
        assert_eq!(links[0].1.map(|id| id.as_uuid()), Some(block));
        assert!(activation.take_focus());

        let _ = handle(&activation, AiChatRequest::Exit);
        assert!(activation.take_exit());
        assert!(!activation.take_exit());
    }

    #[test]
    fn a_bad_link_is_refused_without_navigation() {
        let activation = activation(false);
        assert!(matches!(
            handle(
                &activation,
                AiChatRequest::OpenChat {
                    chat: "not-a-uuid".into(),
                    block: None,
                }
            )
            .0,
            AiChatResponse::Failed { .. }
        ));
        assert!(activation.take_links().is_empty());
    }
}
