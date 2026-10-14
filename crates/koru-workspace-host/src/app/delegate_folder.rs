// SPDX-License-Identifier: MPL-2.0

//! The native picker belongs to the window; its disk probe belongs to a worker.

use super::{App, Host, Path, View, WindowId, mpsc};
use crate::DelegateFolder;

impl<H: Host> App<H> {
    pub(super) fn choose_delegate_folder(
        &mut self,
        source: WindowId,
        title: &str,
        start: Option<&Path>,
    ) {
        let Some(pane) = self.panes.get(&source) else {
            return;
        };
        let Some(folder) = crate::dialogs::pick_folder(title, start, Some(pane.window.as_ref()))
        else {
            return;
        };
        // The folder is a global preference. A later choice in any of this
        // process's windows supersedes the previous check. Cancel changes none.
        self.delegate_folder_check = None;
        let proxy = self.wake_proxy.clone();
        match spawn_check(
            move || {
                if folder_is_writable(&folder) {
                    DelegateFolder::Chosen(folder)
                } else {
                    DelegateFolder::NotWritable
                }
            },
            move || {
                let _ = proxy.send_event(super::Wake::DelegateFolder);
            },
        ) {
            Ok(reply) => {
                self.delegate_folder_check = Some((source, reply));
            }
            Err(error) => {
                tracing::warn!(%error, "could not start delegation folder check");
                self.answer_delegate_folder(source, DelegateFolder::NotWritable);
            }
        }
    }

    pub(super) fn drain_delegate_folder_checks(&mut self) {
        let Some((source, reply)) = self.delegate_folder_check.take() else {
            return;
        };
        if !self.panes.contains_key(&source) {
            return;
        }
        match reply.try_recv() {
            Ok(answer) => self.answer_delegate_folder(source, answer),
            Err(mpsc::TryRecvError::Empty) => {
                self.delegate_folder_check = Some((source, reply));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                tracing::warn!("delegation folder check ended without an answer");
                self.answer_delegate_folder(source, DelegateFolder::NotWritable);
            }
        }
    }

    fn answer_delegate_folder(&mut self, source: WindowId, answer: DelegateFolder) {
        if let Some(pane) = self.panes.get_mut(&source)
            && pane.view.delegate_folder_chosen(answer)
        {
            pane.window.request_redraw();
        }
    }
}

fn spawn_check(
    probe: impl FnOnce() -> DelegateFolder + Send + 'static,
    wake: impl FnOnce() + Send + 'static,
) -> std::io::Result<mpsc::Receiver<DelegateFolder>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("koru-delegate-folder".into())
        .spawn(move || {
            // Queue first, wake second. Dropping a superseded receiver neither
            // waits on the filesystem nor lets its answer reach another picker.
            if tx.send(probe()).is_ok() {
                wake();
            }
        })?;
    Ok(rx)
}

/// Probe the operation the delegate worker needs (ADR-365), on its own thread.
/// Leave the named runs directory behind, but remove only our own probe file.
fn folder_is_writable(base: &Path) -> bool {
    let folder = base.join(koru_settings::RUNS_FOLDER);
    if std::fs::create_dir_all(&folder).is_err() {
        return false;
    }
    // Checks can overlap across windows or superseded picks. A unique name and
    // create_new keep one check from overwriting or removing another's file.
    let probe = folder.join(format!(".koru-writable-{}", uuid::Uuid::new_v4()));
    let Ok(file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    else {
        return false;
    };
    drop(file);
    std::fs::remove_file(&probe).is_ok()
}

#[cfg(test)]
mod tests;
