//! The file list: adding images and folders (dialogs, drops), the pair
//! source, Locate…, and keeping the working version for the next launch.

use std::path::{Path, PathBuf};

use deptheos_app::RawBackend;
use deptheos_app::frames::folder;
use deptheos_app::frames::list::add;
use deptheos_app::frames::list::{Expanded, List};
use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::StackId;
use deptheos_app::library::stacks::Working;
use gpui::prelude::*;
use gpui::{App, Context, ExternalPaths, PathPromptOptions, Task, Window, div};
use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, Sizable as _};

use super::{AppView, Running};
use crate::file_dialog::{beside_last_opened, file_dialog, pick_files, pick_folder, pick_folders};
use crate::look::{Fill, overlay_surface};
use crate::notices::{Action, Kind, Subject, Worded};
use crate::ui_state::{ui_state, update_ui_state};
use crate::words::{Wording as _, say};

/// The working version's write to `list.json` for the next launch.
///
/// Lifecycle: `Idle` with what was kept last, or `Switching` while another
/// stack's is taken in, held by `AppView` and changed only by
/// [`AppView::save_list`] and `switch_to` (`app/navigate.rs`).
///
/// Changes are kept on the store thread (`History::keep_working`), the
/// newest written once it settles. `Idle` → `Switching` when another stack
/// goes on screen; → `Idle` once written, holding the incoming version (or,
/// on failure, the current one kept again), and the screen changes. Nothing
/// else is kept while switching: the switch drops an unwritten keep.
pub(super) enum ListSave {
    /// `kept` is the working version last kept, once this run has.
    Idle { kept: Option<Working> },
    /// The working version on screen kept and another stack's taken in
    /// (`History::switch_stack`); the screen changes once written.
    Switching {
        /// Dropping it drops the landing; the write goes on.
        _task: Task<()>,
    },
}

impl ListSave {
    pub(super) fn is_switching(&self) -> bool {
        matches!(self, Self::Switching { .. })
    }
}

impl AppView {
    /// Another stack is being put on screen ([`ListSave::Switching`]).
    #[cfg(test)]
    pub(crate) fn is_switching(&self) -> bool {
        self.list_save.is_switching()
    }
}

/// A drag of files from outside the window, from its first move over it
/// to its drop or leaving.
///
/// Lifecycle: `None`, `Files` or two or more `Folders`, changed only by
/// [`AppView::set_file_drag`]. `None` → `Files` or `Folders` on the first
/// move, by what it carries ([`FileDrag::of`]); either → `None` on drop or
/// leaving.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum FileDrag {
    #[default]
    None,
    /// Images or folders with images: a drop adds them.
    Files,
    /// Two or more folders only: over the Library, one stack or a batch.
    Folders(Vec<PathBuf>),
}

impl FileDrag {
    /// What a drag of `paths` is. Looks on disk.
    pub(super) fn of(paths: Vec<PathBuf>) -> Self {
        match paths.len() >= 2 && paths.iter().all(|p| p.is_dir()) {
            true => Self::Folders(paths),
            false => Self::Files,
        }
    }

    pub(super) fn is_none(&self) -> bool {
        *self == Self::None
    }
}

pub(crate) use deptheos_app::frames::list::add::AddedFrom;

/// Which of the Library's two targets folders were dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropAs {
    /// One stack of all their frames: what a drop does anywhere else.
    OneStack,
    /// A stack per folder, set up in New Batch.
    Batch,
}

impl AppView {
    pub(super) fn set_file_drag(&mut self, drag: FileDrag, cx: &mut Context<Self>) {
        if self.file_drag != drag {
            tracing::debug!("file drag: {:?} -> {drag:?}", self.file_drag);
            self.file_drag = drag;
            cx.notify();
        }
    }

    /// How many folders get the drop overlay's two targets: two or more over
    /// the Library, nothing running.
    pub(super) fn folder_targets(&self, cx: &App) -> Option<usize> {
        let FileDrag::Folders(folders) = &self.file_drag else { return None };
        let idle = matches!(self.running, super::Running::Idle) && !self.is_processing(cx);
        (self.screen == super::Screen::Library && idle).then_some(folders.len())
    }

    /// Folders dropped on a Library target: one stack, or New Batch with a
    /// stack per folder.
    pub(super) fn drop_folders(
        &mut self,
        on: DropAs,
        dropped: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_file_drag(FileDrag::None, cx);
        let folders = dropped.paths().to_vec();
        tracing::debug!("{} folders dropped as {on:?}", folders.len());
        match on {
            DropAs::OneStack => self.add_paths(folders, window, cx),
            DropAs::Batch => {
                self.open_batch(window, cx);
                if let super::Sheet::Batch(sheet) = &self.sheet {
                    sheet.dialog.update(cx, |d, cx| d.add_folders(folders, cx));
                }
            }
        }
    }

    /// The stack on screen's list and live settings.
    pub(super) fn working(&self, cx: &App) -> Working {
        let list = self.files.read(cx).list().clone();
        Working {
            stack: self.stack.clone(),
            base: self.base.clone(),
            list,
            settings: Some(self.settings(cx)),
        }
    }

    /// New Stack: an empty stack on screen with the current settings; the
    /// one left is kept, so nothing is asked. The only way the window starts
    /// a stack (besides a batch): adding or removing frames stays in it.
    pub(crate) fn new_stack(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_stack_then(window, cx, |_, _, _| {});
    }

    /// [`Self::new_stack`], then `then` once it is on screen (after the
    /// switch is written); never if none goes on screen.
    pub(super) fn new_stack_then(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.is_processing(cx) {
            return;
        }
        self.set_screen(super::Screen::Stacking, window, cx);
        let working = self.working(cx);
        let stacked = self.renders.read(cx).has_renders(&self.stack);
        if working.list.is_empty() && !stacked && self.base.is_none() {
            // A stack with nothing in it is on screen already.
            then(self, window, cx);
            return;
        }
        let fresh = Working::new(working.settings);
        let stack = fresh.stack.clone();
        self.switch_to(
            &stack.clone(),
            Some(fresh),
            window,
            cx,
            move |this, incoming, window, cx| {
                let Some(incoming) = incoming else { return };
                tracing::debug!("new stack {stack:?}");
                this.forget_result(window, cx);
                this.edit(incoming, window, cx);
                then(this, window, cx);
            },
        );
    }

    /// Keep the working version now, on the way out (quit's `work::exit`
    /// writes it). Not while switching: the switch is already queued.
    pub(super) fn write_list_now(&mut self, cx: &mut Context<Self>) {
        if self.list_save.is_switching() {
            return;
        }
        let working = self.working(cx);
        self.renders.read(cx).store().keep_working(working);
    }

    /// Keep the working version for the next launch, on every change to it;
    /// nothing if unchanged, the store writing the newest once it settles.
    /// Not while switching stacks.
    pub(super) fn save_list(&mut self, cx: &mut Context<Self>) {
        // The strip's copy follows every change at once.
        self.sync_on_screen(cx);
        self.sync_library(cx);
        let working = self.working(cx);
        match &self.list_save {
            ListSave::Idle { kept } if kept.as_ref() == Some(&working) => {}
            ListSave::Idle { .. } => {
                tracing::debug!("list save: kept");
                self.renders.read(cx).store().keep_working(working.clone());
                self.list_save = ListSave::Idle { kept: Some(working) };
            }
            ListSave::Switching { .. } => tracing::debug!("list save: not while switching stacks"),
        }
    }

    /// Add…: images and folders from one dialog where the platform has one,
    /// else images (folders are on the button's menu).
    pub(crate) fn add_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.can_select_mixed_files_and_dirs() {
            self.pick_images(window, cx);
            return;
        }
        if self.is_processing(cx) || self.picker.is_open() {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(say!(cx, "list-add-prompt").into()),
        });
        let picked =
            async move { picked.await.ok().and_then(Result::ok).flatten().unwrap_or_default() };
        self.run_dialog(|| picked, window, cx);
    }

    /// Add Frames…: pick image files.
    pub(crate) fn pick_images(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let extensions = deptheos_app::supported_extensions();
        let dialog = self
            .open_dialog_at(window, cx)
            .set_title(say!(cx, "list-add-frames-title"))
            .add_filter(say!(cx, "list-images-filter"), &extensions);
        self.run_dialog(|| pick_files(dialog), window, cx);
    }

    /// Add Folder…: pick folders, each adding the images directly inside.
    pub(crate) fn pick_folders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = self.open_dialog_at(window, cx).set_title(say!(cx, "list-add-folder-title"));
        self.run_dialog(|| pick_folders(dialog), window, cx);
    }

    /// An open dialog starting beside the folder images were last added from.
    fn open_dialog_at(&self, window: &Window, cx: &App) -> rfd::AsyncFileDialog {
        file_dialog(window, beside_last_opened(cx).as_deref())
    }

    /// Run a `pick` dialog and add what it picked, one dialog at a time.
    fn run_dialog<F: std::future::Future<Output = Vec<PathBuf>> + 'static>(
        &mut self,
        pick: impl FnOnce() -> F,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            pick,
            |this, paths, window, cx| this.add_paths(paths, window, cx),
            window,
            cx,
        );
    }

    /// Files dropped on the window: images, and folders (their direct
    /// images). Folders given the Library's two targets but dropped beside
    /// both are not added.
    pub(super) fn drop_files(
        &mut self,
        dropped: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let beside_targets = self.folder_targets(cx).is_some();
        self.set_file_drag(FileDrag::None, cx);
        if beside_targets {
            tracing::debug!("folders dropped beside the Library's targets: not added");
            return;
        }
        self.add_paths(dropped.paths().to_vec(), window, cx);
    }

    /// Add images and folders chosen or dropped ([`Self::add_paths_from`]).
    pub(crate) fn add_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_paths_from(paths, AddedFrom::Window, window, cx);
    }

    /// Add `paths` to the list and show the first, in Stacking (Library
    /// drops too). Refused with a notice while a run runs; settings stay.
    /// `from` says whether the system handed them over, for the Added notice.
    pub(crate) fn add_paths_from(
        &mut self,
        paths: Vec<PathBuf>,
        from: AddedFrom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        if self.is_processing(cx) {
            // Said, never dropped without a word.
            tracing::debug!("add: {} paths refused, something runs", paths.len());
            let message = match self.running {
                super::Running::Batch(_) => say!(cx, "list-not-added-running-batch"),
                super::Running::Sequence { .. } => say!(cx, "list-not-added-running-sequence"),
                super::Running::Stack(_) | super::Running::Idle => {
                    say!(cx, "list-not-added-running-stack")
                }
            };
            self.post(Kind::Warning, Subject::Alone, message, Vec::new(), window, cx);
            return;
        }
        // In the Library no stack is open: adding starts one and lands in it.
        if self.screen == super::Screen::Library {
            self.new_stack_then(window, cx, move |this, window, cx| {
                this.list_into_stack(paths, from, window, cx)
            });
            return;
        }
        self.list_into_stack(paths, from, window, cx);
    }

    /// List `paths` off this thread for the stack on screen; they land
    /// unless another stack is on screen by then ([`Self::land`]).
    fn list_into_stack(
        &mut self,
        paths: Vec<PathBuf>,
        from: AddedFrom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_screen(super::Screen::Stacking, window, cx);
        remember_folder(&paths, cx);
        // Another stack may be on screen when the listing ends.
        let into = self.stack.clone();
        // Listing reads the disk (slow on shares and cards): off this thread.
        cx.spawn_in(window, async move |this, cx| {
            let listed = crate::work::background("list-folder", move || List::expand(paths)).await;
            let expanded = listed.unwrap_or_else(|e| {
                tracing::warn!("listing the added folders failed: {e}");
                Default::default()
            });
            this.update_in(cx, |this, window, cx| this.land(expanded, &into, from, window, cx))
                .ok();
        })
        .detach();
    }

    /// Images listed for `into` landed: add them by [`add::plan`], saying
    /// what couldn't be read and why nothing was added, if so.
    fn land(
        &mut self,
        listed: Expanded,
        into: &StackId,
        from: AddedFrom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = self.files.read(cx).list().clone();
        let now = add::OnScreen {
            stack: &self.stack,
            running: self.is_processing(cx),
            list: &list,
            has_renders: self.renders.read(cx).has_renders(&self.stack),
        };
        let plan = add::plan(listed, into, now, from);
        if !plan.unreadable.is_empty() {
            let w = cx.wording();
            let said: Vec<Worded> = plan
                .unreadable
                .iter()
                .map(|(folder, e)| {
                    let lead = add::unreadable_lead(folder, e, w.shared());
                    let explained = crate::notices::explain(e, w.shared());
                    // Not allowed: said about the folder, with where to allow it.
                    if explained.cause == deptheos_app::ExplanationCause::NotAllowed {
                        let advice = crate::notices::not_allowed_advice(w);
                        return Worded::plain(deptheos_app::words::sentences(
                            w.shared(),
                            [lead.as_str(), &advice],
                        ));
                    }
                    let details = crate::notices::Details::Diagnostics;
                    crate::notices::sentences(w, Some(&lead), &explained, details)
                })
                .collect();
            let message = Worded {
                text: said.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n"),
                fault: said.iter().any(|s| s.fault),
            };
            self.post(Kind::Error, Subject::Alone, message, Vec::new(), window, cx);
        }
        match plan.outcome {
            add::Outcome::Refused(why) => {
                let text = why.text(cx.wording().shared());
                self.post(Kind::Warning, Subject::Alone, text, Vec::new(), window, cx);
            }
            add::Outcome::Nothing => {
                let text = add::nothing_text(cx.wording().shared());
                self.post(Kind::Warning, Subject::Alone, text, Vec::new(), window, cx);
            }
            add::Outcome::Add { files, offer } => self.add_planned(files, offer, window, cx),
        }
    }

    /// Add listed `files` to the stack on screen by the add plan ([`Self::land`]).
    pub(super) fn add_files(
        &mut self,
        files: Vec<PathBuf>,
        from: AddedFrom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let into = self.stack.clone();
        let listed = Expanded { files, unreadable: Vec::new() };
        self.land(listed, &into, from, window, cx);
    }

    /// Add new `files` and show the first; with `offer`, say where they went,
    /// with New Stack Instead.
    fn add_planned(
        &mut self,
        files: Vec<PathBuf>,
        offer: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let added = self.files.update(cx, |list, cx| list.add(files, window, cx));
        let Some(&first) = added.first() else {
            let text = add::nothing_text(cx.wording().shared());
            self.post(Kind::Warning, Subject::Alone, text, Vec::new(), window, cx);
            return;
        };
        // The first added is shown over the render shown, which stays the
        // render shown (ADR-142). Retouch closes: the frame isn't the render's.
        self.leave_paint(cx);
        self.show_file(first, window, cx);
        self.suggest_converter(window, cx);
        if offer {
            self.offer_new_stack(added, window, cx);
        }
    }

    /// "Added 12 frames to canon1", with New Stack Instead.
    fn offer_new_stack(
        &mut self,
        added: Vec<crate::files::FileId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.renders.read(cx).stack_name(&self.stack, cx.wording());
        let message = add::added_text(added.len(), &name, cx.wording().shared());
        let instead = self.action(
            say!(cx, "list-new-stack-instead"),
            move |this, window, cx| this.move_to_new_stack(&added, window, cx),
            cx,
        );
        let subject = Subject::Stack(self.stack.clone());
        self.post(Kind::Info, subject, message, vec![instead], window, cx);
    }

    /// New Stack Instead: move the frames just added into a new stack.
    fn move_to_new_stack(
        &mut self,
        added: &[crate::files::FileId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        let files = self.files.read(cx);
        let paths: Vec<PathBuf> =
            added.iter().filter_map(|id| files.shot_files(*id)).flatten().cloned().collect();
        if paths.is_empty() {
            return;
        }
        self.files.update(cx, |f, cx| f.remove(added, window, cx));
        // Into the new, empty stack: no notice.
        self.new_stack_then(window, cx, move |this, window, cx| {
            this.add_files(paths, AddedFrom::Window, window, cx)
        });
    }

    /// The first time camera RAW is added without Adobe DNG Converter, say
    /// what it adds and where to get it; once ever, and only where Adobe
    /// ships it.
    fn suggest_converter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if ui_state(cx).converter_suggested
            || !self.files.read(cx).has_camera_raw()
            || deptheos_app::converter_executable().is_none()
            || RawBackend::AdobeDng.is_available()
        {
            return;
        }
        update_ui_state(cx, |s| s.converter_suggested = true);
        // Once ever: it stays until closed, so Get It can be reached.
        let get =
            Action::new(say!(cx, "list-get-it"), |_, cx| cx.open_url(deptheos_app::CONVERTER_PAGE));
        let (title, text) =
            (Some("Adobe DNG Converter".into()), say!(cx, "list-converter-suggestion"));
        crate::notices::post_titled(
            Kind::Pinned,
            Subject::Alone,
            title,
            text,
            vec![get],
            window,
            cx,
        );
    }

    /// Show the first included row, else the first.
    pub(super) fn show_first_included(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files.read(cx);
        let included = files.included().first().map(|(id, _)| *id);
        if let Some(id) = included.or_else(|| files.rows().next().map(|(id, _)| id)) {
            self.show_file(id, window, cx);
        }
    }

    /// Use RAW / Use JPEG: which file of each pair the list stacks.
    pub(crate) fn set_source(
        &mut self,
        source: folder::Source,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        self.files.update(cx, |files, cx| files.set_source(source, window, cx));
    }

    /// Locate…: ask where the first missing image's folder went, and relink.
    pub(crate) fn locate_missing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(missing) = self.files.read(cx).first_missing().map(Path::to_path_buf) else {
            return;
        };
        self.locate_folder(&missing, window, cx);
    }

    /// Ask where `missing`'s folder went, and point the list and every
    /// render at it.
    pub(crate) fn locate_folder(
        &mut self,
        missing: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(old) = missing.parent().map(Path::to_path_buf) else { return };
        if self.is_processing(cx) {
            return;
        }
        let title = say!(cx, "list-locate-title", folder = file_label(&old));
        let dialog = self.open_dialog_at(window, cx).set_title(title);
        self.picker.ask(
            |this: &mut Self| &mut this.picker,
            || pick_folder(dialog),
            |this, picked, window, cx| {
                if let Some(new) = picked {
                    this.relink(old, new, window, cx);
                }
            },
            window,
            cx,
        );
    }

    /// Locate…: images under `old` are under `new`. The roots move
    /// ([`crate::renders::Renders::relink`]), then what is open follows
    /// ([`Self::relinked`]).
    pub(crate) fn relink(
        &mut self,
        old: PathBuf,
        new: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_processing(cx) {
            return;
        }
        self.renders.update(cx, |r, cx| r.relink(old, new, window, cx));
    }

    /// The one place what the window has open follows a relink (Locate…, a
    /// root found at launch, an offered place). The list and paint docs
    /// follow the [`deptheos_app::Moves`]; the render shown or opening is
    /// reopened from the Library ([`Self::restore`]) so its frames are read
    /// where they are now. An unsaved result keeps its paths.
    pub(crate) fn relinked(
        &mut self,
        moves: &deptheos_app::Moves,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("relinked: {} folders moved", moves.folders().len());
        self.files.update(cx, |f, cx| f.follow(moves, window, cx));
        self.paint.read(cx).relinked(moves);
        let renders = self.renders.read(cx);
        let reopen =
            renders.pending_id().or_else(|| self.stacking.read(cx).shown_render()).cloned();
        if let Some(render) = reopen {
            self.renders.update(cx, |r, cx| r.open(&render, window, cx));
        }
    }

    /// Say what Locate… found in `folder` ("Found 12 of 14 frames", a warning
    /// while any is missing); nothing if no frame was looked for.
    pub(crate) fn located(
        &mut self,
        folder: &Path,
        matched: deptheos_app::frames::sources::Matched,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matched.of == 0 {
            return;
        }
        let name = file_label(folder);
        let (kind, text) = if matched.found == matched.of {
            (Kind::Done, say!(cx, "list-located-all", n = matched.of, folder = name))
        } else if matched.found == 0 {
            (Kind::Warning, say!(cx, "list-located-none", n = matched.of, folder = name))
        } else {
            let (found, of) = (matched.found, matched.of);
            let text = say!(cx, "list-located-some", found = found, of = of, folder = name);
            (Kind::Warning, text)
        };
        self.post(kind, Subject::Alone, text, Vec::new(), window, cx);
    }

    /// Drop the result on screen: another stack's working version came on
    /// screen, the render shown was deleted, or the list was emptied without
    /// one. Adding or removing frames never does.
    pub(super) fn forget_result(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.leave_paint(cx);
        self.stacking.update(cx, |s, cx| s.clear(cx));
        self.renders.update(cx, |r, cx| r.clear_shown(cx));
        self.viewer.update(cx, |v, cx| v.clear(cx));
        cx.notify();
    }

    /// Over the window while files are dragged in; in the Library, two or
    /// more folders get two targets ([`Self::render_folder_targets`]).
    pub(super) fn render_drop_overlay(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let text = match self.running {
            Running::Stack(_) => say!(cx, "app-drop-while-stack"),
            Running::Batch(_) => say!(cx, "app-drop-while-batch"),
            Running::Sequence { .. } => say!(cx, "app-drop-while-sequence"),
            // A stack's end reaches the view a moment after it stops.
            Running::Idle if self.is_processing(cx) => say!(cx, "app-drop-while-stack"),
            Running::Idle => match self.folder_targets(cx) {
                Some(n) => return self.render_folder_targets(n, cx),
                None => say!(cx, "app-drop-frames"),
            },
        };
        div()
            .absolute()
            .inset_0()
            .p_4()
            .child(
                drop_target(cx)
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(Icon::new(IconName::ImagePlus).large())
                    .child(div().text_lg().child(text)),
            )
            .into_any_element()
    }

    /// Two or more folders over the Library: One Stack, or A Stack per Folder
    /// (opening New Batch). Dropped elsewhere, they are not added.
    fn render_folder_targets(&self, n: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Under the drag the edge turns white: a tinted fill would hide the
        // light text.
        let hover = crate::look::hud_text();
        let target = |id: &'static str, icon: IconName, title: String, detail: String| {
            drop_target(cx)
                .id(id)
                .debug_selector(move || id.into())
                .flex_1()
                .h_full()
                .items_center()
                .justify_center()
                .gap_2()
                .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(hover))
                .child(Icon::new(icon).large())
                .child(div().text_lg().child(title))
                .child(div().text_sm().child(detail))
        };
        let one = target(
            "drop-one-stack",
            IconName::ImagePlus,
            say!(cx, "app-drop-folders-one-stack"),
            say!(cx, "app-drop-folders-one-stack-detail", n = n),
        )
        .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
            this.drop_folders(DropAs::OneStack, paths, window, cx)
        }));
        let batch = target(
            "drop-batch",
            IconName::Layers,
            say!(cx, "app-drop-folders-batch"),
            say!(cx, "app-drop-folders-batch-detail", n = n),
        )
        .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
            this.drop_folders(DropAs::Batch, paths, window, cx)
        }));
        gpui_kit::component::h_flex()
            .absolute()
            .inset_0()
            .p_4()
            .gap_4()
            .child(one)
            .child(batch)
            .into_any_element()
    }
}

/// A drop target: HUD chrome ([`Fill::Hud`]) with a dashed accent edge.
fn drop_target(cx: &App) -> gpui::Div {
    overlay_surface(Fill::Hud, cx)
        .flex()
        .flex_col()
        .border_2()
        .border_dashed()
        .border_color(gpui::rgb(crate::brand::ACCENT_ON_PHOTO))
}

/// Remember where `paths` came from: the open dialog starts there next.
fn remember_folder(paths: &[PathBuf], cx: &mut App) {
    let dir = paths.first().map(|p| if p.is_dir() { p.as_path() } else { p.parent().unwrap_or(p) });
    if let Some(dir) = dir.map(Path::to_path_buf) {
        update_ui_state(cx, |s| s.open_dir = Some(dir));
    }
}
