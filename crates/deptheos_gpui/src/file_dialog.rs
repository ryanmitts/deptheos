//! The one way to open a file dialog: modal to its window, starting in the
//! caller's folder while it exists, else where the system likes. A
//! [`Picker`] keeps one open at a time per owner.

use std::future::Future;
use std::path::{Path, PathBuf};

use gpui::{App, Context, Task, Window};

use crate::ui_state::ui_state;

/// A dialog modal to `window`, opening in `start` if it is a folder that
/// still exists.
pub fn file_dialog(window: &Window, start: Option<&Path>) -> rfd::AsyncFileDialog {
    let dialog = rfd::AsyncFileDialog::new().set_parent(window);
    match start.filter(|dir| dir.is_dir()) {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}

/// The folder beside the last one images came from, so the next card dump
/// is a click away; where add pickers start.
pub fn beside_last_opened(cx: &App) -> Option<PathBuf> {
    let last = ui_state(cx).open_dir?;
    last.ancestors().skip(1).find(|a| a.is_dir()).map(Path::to_path_buf)
}

/// The folder a dialog picked; none when it was canceled.
pub fn pick_folder(dialog: rfd::AsyncFileDialog) -> impl Future<Output = Option<PathBuf>> {
    let picked = dialog.pick_folder();
    async move { picked.await.map(|h| h.path().to_path_buf()) }
}

/// The folders a dialog picked; none when it was canceled.
pub fn pick_folders(dialog: rfd::AsyncFileDialog) -> impl Future<Output = Vec<PathBuf>> {
    let picked = dialog.pick_folders();
    async move { paths_of(picked.await) }
}

/// Where a save dialog says to write; none when it was canceled.
pub fn save_file(dialog: rfd::AsyncFileDialog) -> impl Future<Output = Option<PathBuf>> {
    let picked = dialog.save_file();
    async move { picked.await.map(|h| h.path().to_path_buf()) }
}

/// The files a dialog picked; none when it was canceled.
pub fn pick_files(dialog: rfd::AsyncFileDialog) -> impl Future<Output = Vec<PathBuf>> {
    let picked = dialog.pick_files();
    async move { paths_of(picked.await) }
}

fn paths_of(picked: Option<Vec<rfd::FileHandle>>) -> Vec<PathBuf> {
    picked.unwrap_or_default().iter().map(|h| h.path().to_path_buf()).collect()
}

/// A view's open dialog, one at a time: asking again does nothing, since
/// replacing its task would drop the open dialog's answer.
#[derive(Default)]
pub struct Picker(Option<Task<()>>);

impl Picker {
    pub fn is_open(&self) -> bool {
        self.0.is_some()
    }

    /// Open a `pick` dialog unless one is open, handing its answer to `then`;
    /// `slot` finds this picker in the view to close it.
    pub fn ask<V: 'static, T: 'static, F: Future<Output = T> + 'static>(
        &mut self,
        slot: fn(&mut V) -> &mut Picker,
        pick: impl FnOnce() -> F,
        then: impl FnOnce(&mut V, T, &mut Window, &mut Context<V>) + 'static,
        window: &mut Window,
        cx: &mut Context<V>,
    ) {
        if self.0.is_some() {
            return;
        }
        let picked = pick();
        self.0 = Some(cx.spawn_in(window, async move |this, cx| {
            let picked = picked.await;
            this.update_in(cx, |this, window, cx| {
                slot(this).0 = None;
                then(this, picked, window, cx);
            })
            .ok();
        }));
    }
}
