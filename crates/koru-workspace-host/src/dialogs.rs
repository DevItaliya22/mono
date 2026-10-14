// SPDX-License-Identifier: MPL-2.0

//! Native OS file dialogs, one place.
//!
//! Every picker goes through here so parent-window ownership, filters, and the
//! remembered start directory cannot drift between call sites. Lives in the
//! shell (not `koru-ui`): widgets name no disk and do not depend on `rfd`.

use std::path::{Path, PathBuf};

use rfd::FileDialog;
use winit::window::Window;

/// One named filter: display label + extensions without the dot.
#[derive(Clone, Debug)]
pub struct FileFilter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

/// Build a dialog with optional parent, start directory, and filters.
fn base(
    title: &str,
    start_dir: Option<&Path>,
    parent: Option<&Window>,
    filters: &[FileFilter],
) -> FileDialog {
    let mut dialog = FileDialog::new().set_title(title);
    if let Some(window) = parent {
        dialog = dialog.set_parent(window);
    }
    if let Some(dir) = start_dir.filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    for filter in filters {
        // An empty extension list means "all files", which is what no filter
        // says already; adding it would hide everything on some platforms.
        if !filter.extensions.is_empty() {
            dialog = dialog.add_filter(filter.name, filter.extensions);
        }
    }
    dialog
}

pub fn pick_save_file(
    title: &str,
    file_name: &str,
    filters: &[FileFilter],
    start_dir: Option<&Path>,
    parent: Option<&Window>,
) -> Option<PathBuf> {
    base(title, start_dir, parent, filters)
        .set_file_name(file_name)
        .save_file()
}

pub fn pick_folder(
    title: &str,
    start_dir: Option<&Path>,
    parent: Option<&Window>,
) -> Option<PathBuf> {
    base(title, start_dir, parent, &[]).pick_folder()
}

pub fn pick_open_file(
    title: &str,
    filters: &[FileFilter],
    start_dir: Option<&Path>,
    parent: Option<&Window>,
) -> Option<PathBuf> {
    base(title, start_dir, parent, filters).pick_file()
}

pub fn pick_open_files(
    title: &str,
    filters: &[FileFilter],
    start_dir: Option<&Path>,
    parent: Option<&Window>,
) -> Option<Vec<PathBuf>> {
    let paths = base(title, start_dir, parent, filters).pick_files()?;
    if paths.is_empty() { None } else { Some(paths) }
}
