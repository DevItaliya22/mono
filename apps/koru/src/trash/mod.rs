// SPDX-License-Identifier: MPL-2.0

//! The Trash tab: what is in the bin, and the two things you can do with it.
//!
//! Everything trash-shaped lives under this directory except the two pieces that
//! genuinely belong to their hosts — the pinned row in the explore column
//! (`panes/explore.rs`, which owns that column’s geometry) and the drains in
//! `mainview/trash.rs` (which need a `&mut WorkspaceDoc` a view never holds).
//!
//! A **tab**, on the workspace manager's precedent: it survives a restart, it
//! can sit beside a document, and it is a table — which is a shape nothing else
//! in the window has room for.
//!
//! Built on [`koru_ui::table`], the same component the manager uses. The two
//! screens are the same table with different cells, which is exactly why the
//! table is a component: left alone, each would grow its own copy of six numbers
//! and then drift.

mod draw;
mod menu;
mod view;

pub use menu::{header_menu_at, row_menu_at};
// `Entry` has no caller outside this module yet — the container reads the
// listing through `TrashView` — but it is the shape a row *is*, and the tests
// build them. Public rather than private so that stays true when something else
// wants to describe a trashed row.
#[allow(unused_imports)]
pub use view::Entry;
pub use view::{Request, TrashView};

#[cfg(test)]
mod tests;
