// SPDX-License-Identifier: MPL-2.0

//! Hosts the shared Socials surface as a main-window tab.

use koru_core::schema::WorkspaceDoc;
use koru_draw::geometry::Rect;
use koru_socials_ui::SharedSurface;
use koru_workspace::Workspace;
use koru_workspace_host::View;
use vello::{Scene, kurbo::Affine};
use winit::{
    event::MouseScrollDelta,
    keyboard::{Key, ModifiersState},
    window::CursorIcon,
};

pub struct SocialsView {
    surface: SharedSurface,
}

impl SocialsView {
    pub fn with_surface(surface: SharedSurface) -> Self {
        Self { surface }
    }

    pub fn request_close(&mut self) -> bool {
        self.surface.borrow_mut().request_close()
    }

    pub fn is_dragging(&self) -> bool {
        self.surface.borrow().is_dragging()
    }

    pub fn save_draft(&mut self) -> bool {
        self.surface.borrow_mut().save_draft()
    }

    pub fn import_document(
        &mut self,
        workspace: koru_store_core::WorkspaceId,
        document: koru_store_core::DocumentId,
        title: String,
    ) {
        self.surface
            .borrow_mut()
            .import_document(workspace, document, title);
    }
}

impl View for SocialsView {
    fn key(
        &mut self,
        _ws: &mut WorkspaceDoc,
        key: &Key,
        text: Option<&str>,
        mods: ModifiersState,
    ) -> bool {
        self.surface.borrow_mut().key(key, text, mods)
    }

    fn mouse_down(&mut self, _ws: &mut WorkspaceDoc, x: f32, y: f32, _ctrl: bool) -> bool {
        self.surface.borrow_mut().mouse_down(x, y)
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        self.surface.borrow_mut().mouse_move(x, y)
    }
    fn mouse_up(&mut self, _ws: &mut WorkspaceDoc) -> bool {
        self.surface.borrow_mut().mouse_up()
    }
    fn mouse_double_click(&mut self, _ws: &WorkspaceDoc, x: f32, y: f32) -> bool {
        self.surface.borrow_mut().mouse_double_click(x, y)
    }
    fn scroll_by(&mut self, delta: MouseScrollDelta) {
        self.surface.borrow_mut().scroll_by(delta);
    }
    fn set_viewport(&mut self, w: f32, h: f32) {
        self.surface
            .borrow_mut()
            .set_viewport(Rect::new(0., 0., w, h));
    }
    fn cursor_icon(&self) -> CursorIcon {
        self.surface.borrow().cursor_icon()
    }
    fn adopt(&mut self, _ws: &WorkspaceDoc) {}
    fn reseat(&mut self, _ws: &WorkspaceDoc) {}
    fn invalidate(&mut self) {
        self.surface.borrow_mut().invalidate();
    }

    fn draw(
        &mut self,
        scene: &mut Scene,
        workspace: &Workspace,
        _images: &koru_draw::ImageCache,
        root: Affine,
        _caret_on: bool,
        _pointer: (f32, f32),
    ) {
        let mut surface = self.surface.borrow_mut();
        surface.set_context_workspace(
            workspace
                .current()
                .map(|id| koru_store_core::WorkspaceId::from_uuid(id.as_uuid())),
        );
        surface.draw(scene, root);
    }

    fn maintenance_deadline(&self) -> Option<std::time::Instant> {
        self.surface.borrow().maintenance_deadline()
    }
    fn maintain(&mut self, _ws: &mut WorkspaceDoc, now: std::time::Instant) -> bool {
        self.surface.borrow_mut().maintain(now)
    }
}
