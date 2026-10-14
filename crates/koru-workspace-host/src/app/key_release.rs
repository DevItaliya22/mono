// SPDX-License-Identifier: MPL-2.0

//! **A key coming up, offered to the view.**
//!
//! The release arm of `window_event` settles `down_keys`, the modifiers, the
//! quarantine and a dictation's hold, and until the canvas that was all it did:
//! no release had ever reached a view. A canvas nudges a selected card while an
//! arrow key is held and writes the move once, when the last arrow comes up
//! (CANVAS §4, *one gesture, one request*) — so the release has to arrive
//! somewhere that holds the document.
//!
//! What the view writes is treated the way a key press's write is: the
//! workspace is told it was touched, and every other window on the document
//! adopts it. A view that declines (the default) costs one call.

use winit::keyboard::Key;

use super::{App, Host, View, WindowId};

impl<H: Host> App<H> {
    /// Hand `key`'s release to window `id`'s view, and follow up on what it
    /// wrote.
    pub(super) fn key_released(&mut self, id: WindowId, key: &Key) {
        let before = self.runtime.workspace.ws.version();
        let Some(pane) = self.panes.get_mut(&id) else {
            return;
        };
        if !pane.view.key_up(&mut self.runtime.workspace.ws, key) {
            return;
        }
        if self.runtime.workspace.ws.version() != before {
            self.runtime.workspace.touched();
            self.adopt_others(id);
        }
        self.request_redraw(id);
    }
}
