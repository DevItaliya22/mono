// SPDX-License-Identifier: MPL-2.0

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use alacritty_terminal::event::{Event, EventListener};

use crate::{MAX_EVENTS_PER_BATCH, MAX_TITLE_CHARS};

/// Side effects which terminal output is never allowed to perform directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedAction {
    ClipboardRead,
    ClipboardWrite,
    ColorQuery,
    TextAreaSizeQuery,
    ProcessExit,
}

/// Bounded, host-neutral effects produced while parsing terminal output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmulatorEvent {
    TitleChanged(Option<String>),
    Bell,
    /// A terminal protocol response which the session runtime writes to its PTY.
    PtyWrite(Vec<u8>),
    CursorBlinkingChanged,
    ActionBlocked(BlockedAction),
    /// More events were present in this parse batch than the fixed queue can
    /// represent. The session remains usable; callers may refresh summary UI.
    EventsTruncated,
}

#[derive(Clone, Default)]
pub(crate) struct Listener {
    events: Arc<Mutex<VecDeque<EmulatorEvent>>>,
}

impl Listener {
    fn lock(&self) -> MutexGuard<'_, VecDeque<EmulatorEvent>> {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn drain(&self) -> Vec<EmulatorEvent> {
        self.lock().drain(..).collect()
    }

    fn push(&self, event: EmulatorEvent) {
        let mut events = self.lock();
        if events.len() < MAX_EVENTS_PER_BATCH.saturating_sub(1) {
            events.push_back(event);
        } else if events.back() != Some(&EmulatorEvent::EventsTruncated) {
            events.push_back(EmulatorEvent::EventsTruncated);
        }
    }
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                self.push(EmulatorEvent::TitleChanged(Some(truncate_title(title))));
            }
            Event::ResetTitle => self.push(EmulatorEvent::TitleChanged(None)),
            Event::Bell => self.push(EmulatorEvent::Bell),
            Event::PtyWrite(text) => self.push(EmulatorEvent::PtyWrite(text.into_bytes())),
            Event::CursorBlinkingChange => {
                self.push(EmulatorEvent::CursorBlinkingChanged);
            }
            Event::ClipboardStore(..) => {
                self.push(EmulatorEvent::ActionBlocked(BlockedAction::ClipboardWrite));
            }
            Event::ClipboardLoad(..) => {
                self.push(EmulatorEvent::ActionBlocked(BlockedAction::ClipboardRead));
            }
            Event::ColorRequest(..) => {
                self.push(EmulatorEvent::ActionBlocked(BlockedAction::ColorQuery));
            }
            Event::TextAreaSizeRequest(..) => {
                self.push(EmulatorEvent::ActionBlocked(
                    BlockedAction::TextAreaSizeQuery,
                ));
            }
            Event::Exit | Event::ChildExit(..) => {
                self.push(EmulatorEvent::ActionBlocked(BlockedAction::ProcessExit));
            }
            Event::MouseCursorDirty | Event::Wakeup => {}
        }
    }
}

fn truncate_title(title: String) -> String {
    if title.chars().count() <= MAX_TITLE_CHARS {
        return title;
    }

    title.chars().take(MAX_TITLE_CHARS).collect()
}
