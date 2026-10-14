// SPDX-License-Identifier: MPL-2.0

//! Which menu the Trash tab's two `…` buttons open.
//!
//! **The rows live here; the rendering does not.** Building them on
//! `mainview::context_menu` rather than on a menu of its own is what gives them
//! the red destructive row, the button-anchoring and the disabled-with-a-reason
//! channel for free — and keeps every menu in the window looking like the same
//! menu.

use koru_core::NodeId;

use crate::mainview::context_menu::{MenuAction, MenuSubject};

/// A trashed row's menu: put it back, or really remove it.
pub fn row_menu_at(nodes: Vec<NodeId>) -> (MenuSubject, Vec<MenuAction>) {
    (
        MenuSubject::Trashed(nodes),
        crate::mainview::context_menu::trashed_items(),
    )
}

/// The heading's `…`. One row, and it is the destructive one.
pub fn header_menu_at() -> (MenuSubject, Vec<MenuAction>) {
    (
        MenuSubject::Trash,
        crate::mainview::context_menu::trash_items(),
    )
}
