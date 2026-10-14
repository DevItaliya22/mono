// SPDX-License-Identifier: MPL-2.0

//! Source and inspector windows have separate pointer and layout coordinates.
use super::*;

impl Inspector {
    pub fn panel_viewport(&mut self, width: f32, height: f32) {
        self.detached = true;
        self.bounds = Rect::new(0.0, 0.0, width, height);
        self.clamp_scroll();
        self.dirty_semantics = true;
    }

    pub(super) fn pointer_over_source(&self) -> bool {
        if self.detached {
            self.source_pointer.is_some()
        } else {
            !self.bounds.contains(self.pointer.0, self.pointer.1)
        }
    }

    pub fn panel_event(
        &mut self,
        event: &WindowEvent,
        scale: f64,
        mods: winit::keyboard::ModifiersState,
    ) -> EventResult {
        if matches!(
            event,
            WindowEvent::KeyboardInput {
                is_synthetic: true,
                ..
            }
        ) {
            // A held Enter/Space/Escape replayed on focus is not a control
            // activation either. Genuine repeats retain normal tree navigation.
            return EventResult {
                consumed: true,
                ..Default::default()
            };
        }
        self.source_pointer = None;
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
        ) {
            // The host resized this panel, not the measured source layout.
            return EventResult {
                redraw: true,
                ..Default::default()
            };
        }
        if let WindowEvent::Focused(focused) = event {
            self.focus = *focused;
            self.dirty_semantics = true;
        }
        let mut result = self.event(event, scale, 1.0, mods);
        result.redraw |= matches!(event, WindowEvent::Focused(_));
        // Native window dragging owns screen coordinates; never move the panel
        // rectangle inside its own client area.
        result.drag_window = self.drag.take().is_some();
        result.placement_changed = false;
        self.bounds.x = 0.0;
        self.bounds.y = 0.0;
        result
    }

    pub fn source_event(&mut self, event: &WindowEvent, scale: f64, zoom: f32) -> EventResult {
        let mut result = EventResult::default();
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let pointer = (
                    (position.x / scale) as f32 / zoom,
                    (position.y / scale) as f32 / zoom,
                );
                self.source_pointer = Some(pointer);
                if self.picking {
                    self.select(self.snapshot.pick(pointer.0 as f64, pointer.1 as f64));
                    result.consumed = true;
                    result.redraw = true;
                }
            }
            WindowEvent::CursorLeft { .. } => self.source_pointer = None,
            WindowEvent::MouseInput { state, .. } => {
                result.consumed = self.picking || self.captured_press;
                self.captured_press = state.is_pressed() && result.consumed;
                if state.is_pressed() && self.picking {
                    self.pause_refresh();
                    self.dirty_semantics = true;
                    result.redraw = true;
                } else if state.is_pressed() {
                    self.invalidate_selection();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if self.picking => {
                result.consumed = true;
                if event.state.is_pressed() && event.logical_key == Key::Named(NamedKey::Escape) {
                    self.pause_refresh();
                    result.redraw = true;
                }
            }
            WindowEvent::Ime(_) if self.picking => result.consumed = true,
            WindowEvent::Focused(_) => {
                self.focus = false;
                self.captured_press = false;
            }
            WindowEvent::MouseWheel { .. }
            | WindowEvent::KeyboardInput { .. }
            | WindowEvent::Resized(_)
            | WindowEvent::ScaleFactorChanged { .. } => {
                self.invalidate_selection();
            }
            _ => {}
        }
        result
    }
}
