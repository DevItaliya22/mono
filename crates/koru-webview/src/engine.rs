// SPDX-License-Identifier: MPL-2.0

//! The wry-backed surface. The only file in the workspace that names a wry type.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wry::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder};

use crate::{PhysicalRect, WebConfig, WebError, WebEvent, debug, decide, host_of};

pub(crate) struct Inner {
    // Declared first so it is dropped first: the view must go before the
    // context it was created in, and both before the parent window.
    webview: WebView,
    _context: WebContext,
    #[cfg(windows)]
    parent_hwnd: isize,
}

fn rect(bounds: PhysicalRect) -> Rect {
    Rect {
        position: Position::Physical(PhysicalPosition::new(bounds.x, bounds.y)),
        size: Size::Physical(PhysicalSize::new(bounds.width, bounds.height)),
    }
}

fn call(result: wry::Result<()>) -> Result<(), WebError> {
    result.map_err(|e| WebError::Call(e.to_string()))
}

/// A sender and a waker, cloned into every engine callback.
#[derive(Clone)]
struct Outbox {
    sender: Sender<WebEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Outbox {
    fn push(&self, event: WebEvent) {
        if self.sender.send(event).is_ok() {
            (self.wake)();
        }
    }
}

impl Inner {
    pub(crate) fn open(
        parent: &impl HasWindowHandle,
        bounds: PhysicalRect,
        config: WebConfig,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(Self, Receiver<WebEvent>), WebError> {
        #[cfg(windows)]
        let parent_hwnd = match parent.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
            Ok(_) => return Err(WebError::Create("not a Win32 window".into())),
            Err(e) => return Err(WebError::Create(format!("no window handle: {e}"))),
        };

        let (sender, receiver) = mpsc::channel();
        let outbox = Outbox { sender, wake };
        let mut context = WebContext::new(Some(config.data_directory.clone()));
        let policy = config.policy;
        let first = host_of(&config.url).to_string();
        debug::log("create", "begin", &first);
        let started = std::time::Instant::now();

        let (nav, ipc, popup, load) = (outbox.clone(), outbox.clone(), outbox.clone(), outbox);
        let mut builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(&config.url)
            .with_bounds(rect(bounds))
            .with_devtools(config.devtools)
            .with_navigation_handler(move |url| {
                let (allow, event) = decide(&*policy, &url);
                debug::log(
                    "navigate",
                    match &event {
                        None => "allow",
                        Some(WebEvent::External { .. }) => "external",
                        Some(_) => "refuse",
                    },
                    host_of(&url),
                );
                if let Some(event) = event {
                    nav.push(event);
                }
                allow
            })
            .with_ipc_handler(move |request| {
                let source = request.uri().to_string();
                ipc.push(WebEvent::Ipc {
                    body: request.into_body(),
                    source,
                });
            })
            .with_new_window_req_handler(move |url, _features| {
                debug::log("new-window", "deny", host_of(&url));
                popup.push(WebEvent::NewWindow { url });
                NewWindowResponse::Deny
            })
            .with_on_page_load_handler(move |event, url| {
                let (stage, event) = match event {
                    PageLoadEvent::Started => ("page", WebEvent::PageStarted { url: url.clone() }),
                    PageLoadEvent::Finished => {
                        ("page", WebEvent::PageFinished { url: url.clone() })
                    }
                };
                debug::log(
                    stage,
                    if matches!(event, WebEvent::PageStarted { .. }) {
                        "started"
                    } else {
                        "finished"
                    },
                    host_of(&url),
                );
                load.push(event);
            });
        if let Some(suffix) = config.user_agent_suffix.as_deref() {
            builder = builder.with_user_agent(format!("{} {suffix}", default_user_agent()));
        }
        let webview = builder.build_as_child(parent).map_err(|e| {
            debug::log("create", "failed", &first);
            classify_create_error(e)
        })?;
        debug::log(
            "create",
            &format!("ok {}ms", started.elapsed().as_millis()),
            &first,
        );
        Ok((
            Self {
                webview,
                _context: context,
                #[cfg(windows)]
                parent_hwnd,
            },
            receiver,
        ))
    }

    pub(crate) fn set_bounds(&self, bounds: PhysicalRect) -> Result<(), WebError> {
        call(self.webview.set_bounds(rect(bounds)))
    }

    pub(crate) fn set_visible(&self, visible: bool) -> Result<(), WebError> {
        call(self.webview.set_visible(visible))
    }

    pub(crate) fn focus(&self) -> Result<(), WebError> {
        call(self.webview.focus())
    }

    pub(crate) fn focus_parent(&self) -> Result<(), WebError> {
        call(self.webview.focus_parent())
    }

    pub(crate) fn eval(&self, js: &str) -> Result<(), WebError> {
        call(self.webview.evaluate_script(js))
    }

    pub(crate) fn navigate(&self, url: &str) -> Result<(), WebError> {
        call(self.webview.load_url(url))
    }

    pub(crate) fn clear_all_browsing_data(&self) -> Result<(), WebError> {
        call(self.webview.clear_all_browsing_data())
    }

    /// Windows: the focused HWND is the child, or a descendant of it. The
    /// parent window has no other children, so "focus is somewhere inside the
    /// parent other than the parent itself" is the same question.
    #[cfg(windows)]
    pub(crate) fn has_focus(&self) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus;
        use windows_sys::Win32::UI::WindowsAndMessaging::IsChild;
        // SAFETY: both are plain queries on window handles; a stale handle
        // yields null / false, never a fault.
        unsafe {
            let focused = GetFocus();
            if focused.is_null() || focused as isize == self.parent_hwnd {
                return false;
            }
            IsChild(self.parent_hwnd as _, focused) != 0
        }
    }

    /// macOS: the window's first responder is the WKWebView or one of its
    /// descendants. Written against objc2-app-kit 0.3 and **not compiled on
    /// the machine that wrote it**; the companion plan's M1 builds it first.
    #[cfg(target_os = "macos")]
    pub(crate) fn has_focus(&self) -> bool {
        use objc2_app_kit::NSView;
        use wry::WebViewExtMacOS;
        let view = self.webview.webview();
        let window = self.webview.ns_window();
        let Some(responder) = window.firstResponder() else {
            return false;
        };
        match responder.downcast_ref::<NSView>() {
            Some(focused) => focused.isDescendantOf(&view),
            None => false,
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub(crate) fn has_focus(&self) -> bool {
        false
    }
}

/// wry's own default UA is not exposed; the engine substitutes its own when
/// the string is empty, so the suffix is appended to a marker the inbox can
/// read rather than to a copy of the engine's string.
fn default_user_agent() -> &'static str {
    "KoruDesktop"
}

/// The one failure worth telling apart: no runtime on the machine, which is a
/// browser-fallback case rather than a bug.
fn classify_create_error(error: wry::Error) -> WebError {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("runtime") || lower.contains("webview2") && lower.contains("not") {
        WebError::RuntimeUnavailable(text)
    } else {
        WebError::Create(text)
    }
}
