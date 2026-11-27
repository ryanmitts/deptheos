//! The application view: lays out the sidebar and the viewer, and connects
//! the entities that hold the state:
//!
//! - [`FileList`]: the images added, which are included, which is shown.
//! - [`StackParams`]: algorithm, sliders, alignment, auto-crop.
//! - [`Stacking`]: the run, its progress and the result (a model).
//! - [`Viewer`]: the images, pan/zoom, compare modes and their loads.
//! - [`Renders`]: every finished stack, listed in the strip.
//! - [`Paint`]: paint mode, its brush, source and worker.
//! - [`Sequences`](crate::sequences::Sequences): grow and turntable
//!   sequences being written (a model).
//!
//! This view turns each one's events into calls on the others and handles
//! the window's actions. Blocking work goes through [`crate::work`]
//! (ADR-143), awaited by a task owned by the entity it updates.
//!
//! Its work is split into `impl AppView` blocks by subject in `app/`: `list`,
//! `run`, `batch_run`, `batch_after`, `sequence`, `navigate`, `export`,
//! `exporting`, `library_export`, `mode`, `paint_mode`, `retouch`, `view`,
//! `view_bar`, `actions`, `bindings`, `menu`, `window_menu`, `toolbar`,
//! `welcome`, `quit`, `update_check`, `diagnostics`, `chrome`, `sidebar`,
//! `sheet`, `scrubber`, `working`, `sample`, `library_delete`, `away` and
//! `command_line_tool`.

pub mod actions;
pub(crate) mod away;
mod batch_after;
mod batch_run;
mod bindings;
pub(crate) mod chrome;
mod command_line_tool;
pub(crate) mod diagnostics;
mod export;
mod exporting;
pub(crate) mod library_delete;
mod library_export;
mod list;
mod menu;
mod mode;
mod navigate;
mod paint_mode;
pub(crate) mod quit;
pub(crate) mod retouch;
mod run;
mod sample;
mod scrubber;
mod sequence;
mod sheet;
mod sidebar;
pub(crate) use sheet::{ModalDialog, Sheet};
use sidebar::SidebarResize;
#[cfg(test)]
mod tests;
mod toolbar;
mod update_check;
mod view;
mod view_bar;
mod welcome;
pub(crate) mod window_menu;
mod working;
pub(crate) use working::Snapshot;
use working::TitleParts;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use deptheos_app::Engine;
use deptheos_app::library::history::{Batch, RenderId, StackId};
use deptheos_app::library::stacks::{self, Working};
use deptheos_app::runs::batch;
use gpui::prelude::*;
use gpui::{
    App, Context, DragMoveEvent, Entity, ExternalPaths, FocusHandle, Focusable, Pixels,
    Subscription, Task, Window, div, px,
};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::InputState;

use crate::files::{FileList, FileListEvent};
use crate::notices::{Kind, Subject};
use crate::paint::{Paint, PaintEvent};
use crate::params::{ParamsEvent, StackParams};
use crate::prefs::{Prefs, prefs};
use crate::renders::{CurrentStack, Library, LibraryEvent, Renders, RendersEvent, UnstackedNow};
use crate::stacking::{Stacking, StackingEvent};
use crate::ui_state::{SavedWindow, ui_state, update_ui_state};
use crate::viewer::{Loader, Source, Viewer, ViewerEvent};
use crate::words::{Wording as _, say};
use actions::*;
use chrome::WorkspaceChrome;
pub(crate) use list::AddedFrom;
pub(crate) use sequence::frame_format;

use deptheos_app::library::stacks::Settings;

/// A notice's inset from the window's right edge and below the title bar.
const NOTICE_INSET: f32 = 12.0;

/// Notices sit top right below the title bar, clear of its buttons (the
/// kit measures from its own, shorter title bar). There they cover the
/// Method picker, so a stack that went as asked ends in the inspector's
/// footer instead (`run::LastStack`). Set once with the app's theme.
pub(crate) fn place_notices(cx: &mut App) {
    let notices = &mut gpui_kit::component::Theme::global_mut(cx).notification;
    notices.placement = gpui::Anchor::TopRight;
    notices.margins = gpui_kit::base::Edges {
        top: px(toolbar::TITLE_BAR_HEIGHT + NOTICE_INSET),
        right: px(NOTICE_INSET),
        bottom: px(NOTICE_INSET),
        left: px(NOTICE_INSET),
    };
}

/// A batch run: stacks made one after another with the settings as they
/// were at its start.
struct BatchRun {
    batch: Batch,
    settings: Settings,
    /// The stack each job's render is saved into, by job index
    /// ([`batch::Action::RecordUnstacked`]).
    stacks: Vec<StackId>,
    /// Where the run is, and what is next.
    machine: batch::Machine,
    /// What the desktop keeps of its stacks, exports and sequence.
    after: batch_after::AfterRun,
    /// The Mac kept awake while it lasts.
    _awake: run::Awake,
}

/// What the stack running is saved as when it finishes.
struct PendingRender {
    /// The list its render records.
    listed: Vec<PathBuf>,
    /// The stack it joins.
    of: StackId,
    /// The base the working version has as it starts.
    from: Option<RenderId>,
    /// What the viewer showed at the start, shown again on cancel.
    shown_before: run::ShownBefore,
}

/// What is running in the window: one thing at a time, and the list,
/// settings and history are locked while it runs.
///
/// Lifecycle: `Idle`, a `Stack`, a grow `Sequence` or a `Batch` (the last
/// two holding an `Awake`), held by `AppView`, changed only by
/// [`AppView::set_running`] (`app/run.rs`); the lock is this not being `Idle`.
///
/// ```text
///        ┌── Stack ─────── the stack finished, failed or was canceled ──┐
/// Idle ──┼── Sequence ──── the grow sequence ended ──────────────────────┼──▶ Idle
///        └── Batch ─────── the run's machine said Finish ────────────────┘
/// ```
///
/// A batch stays `Batch` until its closing notice, through saves, exports
/// and its turntable sequence.
enum Running {
    Idle,
    /// One stack, of the stack on screen: what its render records.
    Stack(PendingRender),
    /// A grow sequence of a render, the Mac kept awake while it lasts.
    Sequence {
        _awake: run::Awake,
    },
    Batch(Box<BatchRun>),
}

impl Running {
    fn name(&self) -> &'static str {
        match self {
            Running::Idle => "idle",
            Running::Stack(_) => "stack",
            Running::Sequence { .. } => "sequence",
            Running::Batch(_) => "batch",
        }
    }
}

pub struct AppView {
    focus_handle: FocusHandle,
    pub(crate) files: Entity<FileList>,
    pub(crate) params: Entity<StackParams>,
    pub(crate) stacking: Entity<Stacking>,
    viewer: Entity<Viewer>,
    pub(crate) renders: Entity<Renders>,
    /// The Library screen: every render, by day and batch.
    pub(crate) library: Entity<Library>,
    /// Which screen is shown (⌥⌘1, ⌥⌘2).
    pub(crate) screen: Screen,
    /// How the last stack ended, for the inspector.
    last_stack: run::LastStack,
    pub(crate) paint: Entity<Paint>,
    /// Viewing, Retouch or Adjust. Changed only by [`Self::set_mode`].
    mode: mode::Mode,
    /// The window's Adjust session.
    adjust: mode::AdjustHandle,
    /// What is running, if anything: a stack, a sequence or a batch.
    running: Running,
    /// The stack on screen, which Stack adds its next render to.
    stack: StackId,
    /// The render the working version started from: its base.
    base: Option<RenderId>,
    /// The sheet over the window. Changed only by [`Self::set_sheet`].
    pub(crate) sheet: Sheet,
    /// Tests hold a batch's exports and count them.
    #[cfg(test)]
    pub(crate) export_probe: batch_after::ExportProbe,
    /// Grow and turntable sequences being written.
    pub(crate) sequences: Entity<crate::sequences::Sequences>,
    /// The window's one export. Changed only by `set_exporting`.
    exporting: exporting::Exporting,
    /// What the menu bar says now.
    menu_state: MenuState,
    /// The ≡ menu (Windows and Linux) while open.
    window_menu: Option<(Entity<gpui_kit::component::menu::PopupMenu>, Subscription)>,
    /// Alt is down alone (no other key or press): releasing it opens ≡.
    alt_down: bool,
    /// The file dialog open, if any.
    picker: crate::file_dialog::Picker,
    /// The working version being written for the next launch.
    list_save: list::ListSave,
    /// The update check in flight.
    update_check: Option<Task<()>>,
    /// Where a quit is: changed only in `app/quit.rs`.
    quitting: quit::Quitting,
    /// The sidebar's width, set by dragging its edge.
    sidebar_width: Pixels,
    /// The panels, Focus Mode and window width. Changed only by
    /// [`Self::set_chrome`].
    chrome: WorkspaceChrome,
    /// The focus rail's pointer, in the view bar.
    rail: scrubber::Rail,
    /// The window's title as last set: what is open, then the app's name.
    window_title: gpui::SharedString,
    /// Files from outside the app being dragged over the window.
    file_drag: list::FileDrag,
    /// The zoom field: the zoom while unfocused, a percentage to go to
    /// while focused.
    pub(crate) zoom_input: Entity<InputState>,
    /// The sample stacks this build carries; with none, Try a Sample Stack
    /// opens the website's samples.
    pub(crate) samples: Vec<Arc<sample::Sample>>,
    /// Changed only by [`Self::set_sample_open`].
    sample_open: sample::SampleOpen,
    _subscriptions: Vec<Subscription>,
}

/// The window's two screens: the Library (home) and Stacking (one stack).
///
/// Lifecycle: `Library` or `Stacking`, held by `AppView`, changed only by
/// `set_screen` (`app/navigate.rs`). `Library` → `Stacking`: a stack
/// opened, New Stack, frames added, a batch started. `Stacking` →
/// `Library`: "‹ Library". Both by ⌥⌘1 and ⌥⌘2.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Every stack, by day and batch.
    #[default]
    Library,
    /// The frames, the photo and its settings.
    Stacking,
}

/// The inspector's width, in points.
const INSPECTOR_WIDTH: f32 = 272.0;

impl AppView {
    /// The window's view over `engine` and the Library `opened`: the list
    /// left last time, then `paths`.
    pub fn new(
        engine: Arc<Engine>,
        paths: Vec<PathBuf>,
        opened: crate::start::Opened,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // GPUI focuses only on mouse-down; without this, key bindings and
        // the File menu are dead at launch.
        window.focus(&focus_handle, cx);

        let crate::start::Opened { history: store, working: saved } = opened;
        // Bookmarks follow a root folder renamed or moved on its volume.
        if cfg!(target_os = "macos") {
            store.frames().make_bookmarks_with(crate::bookmarks::make);
        }
        let store = Arc::new(store);
        let Working { stack, base, list: saved_list, settings: saved_settings } = saved;
        // The list asks the roots whether a frame is missing or offline.
        let frames = store.frames().clone();
        let files = cx.new(|cx| FileList::new(frames, window, cx));
        let params = cx.new(|cx| StackParams::new(files.clone(), window, cx));
        let stacking = cx.new(|_| Stacking::new(engine.clone()));
        let sequences = cx.new(|_| crate::sequences::Sequences::new(engine.clone()));
        let renders =
            cx.new(|cx| Renders::new(engine.clone(), store.clone(), stacking.clone(), window, cx));
        let library = cx.new(|cx| Library::new(renders.clone(), window, cx));
        let viewer = cx.new(|cx| Viewer::new(engine_loader(engine.clone()), cx));
        let adjust = mode::AdjustHandle::new(engine.clone(), store.clone(), window, cx);
        let paint = cx.new(|cx| Paint::new(engine.clone(), viewer.clone(), store, cx));
        engine.set_camera_profile(prefs(cx).camera_profile);
        let zoom_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("–").validate(|text, _| {
                // Digits and %, or "Fit", which it shows when fitted.
                text.len() <= 6 && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '%')
            })
        });
        let _subscriptions = vec![
            // A focused control gone (Method segments as Retouch opens, a
            // row deleted): keys return to the app, or Retouch's keys, Escape
            // and ⌘Z do nothing until a click.
            cx.on_focus_lost(window, |this, window, cx| {
                tracing::debug!("focus lost: back to the window");
                window.focus(&this.focus_handle, cx);
            }),
            cx.subscribe_in(&files, window, Self::on_file_event),
            cx.subscribe_in(&stacking, window, Self::on_stacking_event),
            cx.subscribe_in(&sequences, window, Self::on_sequence_event),
            cx.observe(&sequences, |_, _, cx| cx.notify()),
            cx.subscribe_in(&viewer, window, Self::on_viewer_event),
            cx.subscribe_in(&renders, window, Self::on_renders_event),
            cx.subscribe_in(&library, window, |this, _, event: &LibraryEvent, window, cx| {
                match event {
                    LibraryEvent::Open(id) => this.open_from_library(id.clone(), window, cx),
                    LibraryEvent::OpenStack(stack) => {
                        this.open_stack_from_library(stack, window, cx)
                    }
                    LibraryEvent::Export(id) => this.export_from_library(id.clone(), window, cx),
                }
            }),
            cx.subscribe_in(&paint, window, Self::on_paint_event),
            cx.subscribe_in(
                &params,
                window,
                |this, _, event: &ParamsEvent, window, cx| match event {
                    ParamsEvent::Revert => this.revert_shown(window, cx),
                    // Survive a crash with the settings changed.
                    ParamsEvent::Changed => this.save_list(cx),
                },
            ),
            // The strip follows the history, and so does the Library's
            // open stack.
            cx.observe(&renders, |this, _, cx| {
                this.sync_library(cx);
                cx.notify()
            }),
            // The paint panel and the list's painted marks follow the worker.
            cx.observe(&paint, |this, _, cx| {
                this.sync_painted(cx);
                cx.notify()
            }),
            // Source previews color camera RAW the way stacks will.
            cx.observe_global::<Prefs>(move |_, cx| {
                engine.set_camera_profile(prefs(cx).camera_profile);
            }),
            // A key released while another window had focus never arrives:
            // space-pan and hold-for-source end with the focus.
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if !window.is_window_active() {
                    this.viewer.update(cx, |v, cx| v.release_keys(cx));
                }
            }),
            // Remembered for the next launch.
            cx.observe_window_bounds(window, |_, window, cx| {
                let saved = SavedWindow::from_bounds(window.window_bounds());
                update_ui_state(cx, |s| s.window = Some(saved));
            }),
            cx.subscribe_in(&zoom_input, window, Self::on_zoom_input_event),
        ];

        let mut this = Self {
            focus_handle,
            files,
            params,
            stacking,
            viewer,
            renders,
            library,
            // The Library is home: a stack is opened from it.
            screen: Screen::Library,
            last_stack: run::LastStack::Nothing,
            paint,
            mode: mode::Mode::Viewing,
            adjust,
            running: Running::Idle,
            stack,
            base,
            sheet: Sheet::None,
            #[cfg(test)]
            export_probe: Default::default(),
            sequences,
            exporting: exporting::Exporting::Idle,
            menu_state: MenuState::default(),
            window_menu: None,
            alt_down: false,
            picker: Default::default(),
            list_save: list::ListSave::Idle { kept: None },
            update_check: None,
            quitting: quit::Quitting::Staying,
            sidebar_width: px(ui_state(cx).sidebar_width()),
            rail: scrubber::Rail::default(),
            chrome: WorkspaceChrome::restored(
                ui_state(cx).sidebar_hidden,
                ui_state(cx).inspector_hidden,
            ),
            window_title: say!(cx, "window-title").into(),
            file_drag: list::FileDrag::None,
            zoom_input,
            samples: sample::bundled(),
            sample_open: sample::SampleOpen::Idle,
            _subscriptions,
        };
        if let Some(Settings { config, options }) = saved_settings {
            this.params.update(cx, |p, cx| p.restore(&config, &options, window, cx));
        }
        if !saved_list.is_empty() {
            this.files.update(cx, |f, cx| f.set_list(saved_list, window, cx));
            this.show_first_included(window, cx);
        }
        this.sync_library(cx);
        // The stack left open last time is the one selected, so ↩ opens it.
        let open = this.stack.clone();
        this.library.update(cx, |l, cx| l.select_open(open, cx));
        // Command-line image paths start a stack, the first shown.
        this.add_paths_from(paths, list::AddedFrom::Outside, window, cx);
        // Closing the window quits: ask first during a run.
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| this.confirm_quit(window, cx)).unwrap_or(true)
        });
        this
    }

    /// Something runs or is still winding down: blocks a new stack, an
    /// open, a Revert or a list edit. See also [`Self::entities_running`],
    /// [`Self::can_cancel`], [`Self::busy_reason`] and [`Self::batch_run`].
    pub(crate) fn is_processing(&self, cx: &App) -> bool {
        // The entities too: a stack's end arrives a moment after the stack
        // stops, and a switch of stacks being written blocks too.
        !matches!(self.running, Running::Idle)
            || self.entities_running(cx)
            || self.list_save.is_switching()
    }

    /// The stacker or a sequence is at work, whatever [`Running`] says: not
    /// between a batch's stacks, and still a moment after a stack ends.
    fn entities_running(&self, cx: &App) -> bool {
        self.stacking.read(cx).is_running() || self.sequences.read(cx).is_running()
    }

    pub(crate) fn has_result(&self, cx: &App) -> bool {
        self.stacking.read(cx).result().is_some()
    }

    // ------------------------------------------------------------------
    // Events from the entities
    // ------------------------------------------------------------------

    fn on_file_event(
        &mut self,
        _: &Entity<FileList>,
        event: &FileListEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FileListEvent::Selected(id) => self.show_file(*id, window, cx),
            FileListEvent::Changed => {
                self.save_list(cx);
            }
            // Every frame gone; the stack stays (only New Stack starts one).
            // A render shown stays, and Revert brings its frames back.
            FileListEvent::Cleared => {
                self.save_list(cx);
                if self.snapshot(cx).shown.is_none() {
                    self.forget_result(window, cx);
                }
            }
        }
    }

    fn on_stacking_event(
        &mut self,
        stacking: &Entity<Stacking>,
        event: &StackingEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            StackingEvent::LiveMap => {
                if let Some(map) = stacking.update(cx, |s, _| s.take_live_map()) {
                    self.viewer.update(cx, |v, cx| v.show_live_map(map, cx));
                }
            }
            StackingEvent::Finished { seconds } => {
                self.stack_finished(stacking, *seconds, window, cx)
            }
            StackingEvent::Canceled => self.stack_canceled(window, cx),
            StackingEvent::Failed(error) => self.stack_failed(error, window, cx),
            StackingEvent::Shown => self.sync_left_out(cx),
        }
    }

    fn on_renders_event(
        &mut self,
        renders: &Entity<Renders>,
        event: &RendersEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Starting a stack cancels an open in flight (`set_locked`).
            RendersEvent::Opened => {
                if let Some(opened) = renders.update(cx, |r, _| r.take_opened()) {
                    self.restore(opened, window, cx);
                }
            }
            RendersEvent::Compare(render) => self.compare_with_render(render.clone(), window, cx),
            RendersEvent::Saved { stack, render, from } => {
                // A save landing after another open doesn't take the title
                // or the active tile.
                let shown =
                    self.stacking.update(cx, |s, cx| s.mark_saved(stack, render.clone(), cx));
                self.rebase_on_saved(render, from.as_ref(), cx);
                if shown {
                    self.log_shown(cx);
                }
                self.batch_render_saved(stack, Ok(render), window, cx);
                // The Paint tab may open now.
                cx.notify();
            }
            // The stack on screen was deleted: an empty one replaces it,
            // with the current settings.
            RendersEvent::StacksDeleted(stacks) => {
                if stacks.contains(&self.stack) {
                    let settings = self.working(cx).settings;
                    self.forget_result(window, cx);
                    self.edit(deptheos_app::library::stacks::Working::new(settings), window, cx);
                }
            }
            RendersEvent::LibraryDeleted => self.library_deleted(window, cx),
            RendersEvent::MovedToDeleted(deletion) => self.say_deleted(deletion, window, cx),
            RendersEvent::Deleted { ids, compared } => {
                // A compare against a render that is gone ends with it.
                if *compared {
                    self.viewer.update(cx, |v, cx| v.clear_compare(cx));
                }
                // The base is gone: the stack's newest render left becomes
                // it, so it still reads "Edited since #n".
                if self.base.as_ref().is_some_and(|b| ids.contains(b)) {
                    let renders = self.renders.read(cx).renders();
                    self.base = stacks::newest_of(&self.stack, renders).map(|r| r.id.clone());
                    self.save_list(cx);
                }
                // A deleted render's picture goes with it.
                if self.stacking.read(cx).shown_render().is_some_and(|r| ids.contains(r)) {
                    self.leave_paint(cx);
                    self.show_own_result(window, cx);
                }
            }
            RendersEvent::NotSaved { stack, message } => {
                self.batch_render_saved(stack, Err(message.to_string()), window, cx)
            }
            RendersEvent::Failed { message, about } => {
                self.post(Kind::Error, about.clone(), message.clone(), Vec::new(), window, cx)
            }
            RendersEvent::Retry(failure) => self.retry(failure, window, cx),
            RendersEvent::Revert(render) => self.revert_to(render, window, cx),
            RendersEvent::Relinked(moves) => self.relinked(moves, window, cx),
            RendersEvent::Located { folder, matched } => self.located(folder, *matched, window, cx),
            RendersEvent::ExportGrow(id) => self.export_grow_sequence(Some(id.clone()), window, cx),
        }
    }

    /// Announce a delete into Recently Deleted, with Undo for exactly these
    /// records; a newer delete's notice replaces it.
    fn say_deleted(
        &mut self,
        deletion: &deptheos_app::library::history::Deletion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let untitled = say!(cx, "app-untitled-stack");
        let text = deptheos_app::library::history::deleted_notice(
            crate::words::of(cx),
            deletion,
            &untitled,
        );
        let ids = deletion.ids();
        let undo = self.action(
            say!(cx, "renders-deleted-undo"),
            move |this, window, cx| {
                let ids = ids.clone();
                this.renders.update(cx, |r, cx| r.restore(ids, window, cx));
            },
            cx,
        );
        self.post(Kind::Info, Subject::Deleted, text, vec![undo], window, cx);
    }

    fn on_viewer_event(
        &mut self,
        _: &Entity<Viewer>,
        event: &ViewerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ViewerEvent::LoadFailed { file, message } => {
                match file {
                    // Unhighlight it, so compare modes pick another source
                    // instead of retrying this one.
                    Some(file) => {
                        if self.files.read(cx).selected(cx) == Some(*file) {
                            self.files.update(cx, |files, cx| files.highlight(None, window, cx));
                        }
                    }
                    // A render compare: nothing is pinned after all.
                    None => self.renders.update(cx, |r, cx| r.clear_compare(cx)),
                }
                let w = cx.wording().clone();
                let text = message.clone().reworded(|reason| match file {
                    Some(_) => say!(w, "app-frame-not-shown", reason = reason),
                    None => say!(w, "app-compare-not-shown", reason = reason),
                });
                self.post(Kind::Error, Subject::Alone, text, Vec::new(), window, cx);
            }
            ViewerEvent::Paint(input) => {
                let input = *input;
                self.paint.update(cx, |p, cx| p.input(input, cx));
            }
            ViewerEvent::PickAt { x, y } => self.pick_sharpest(*x, *y, window, cx),
            ViewerEvent::MapPointer(at) => self.point_at_map(*at, cx),
            ViewerEvent::MapClick { x, y } => self.click_map(*x, *y, window, cx),
        }
    }

    fn on_paint_event(
        &mut self,
        _: &Entity<Paint>,
        event: &PaintEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PaintEvent::Failed(message) => {
                self.post(Kind::Error, Subject::Alone, message.clone(), Vec::new(), window, cx)
            }
            PaintEvent::Notice(text) => {
                self.post(Kind::Info, Subject::Alone, text.clone(), Vec::new(), window, cx)
            }
            PaintEvent::Source { key, image } => {
                if let Some(image) = image.take() {
                    self.show_source(key, image, cx);
                }
            }
            // No display: the render shows its plain result, as it does.
            PaintEvent::Presented { key, display } => {
                let current = self.result_doc(cx).is_some_and(|(_, k)| k == *key);
                if current
                    && !self.is_retouching()
                    && !self.is_processing(cx)
                    && let Some(display) = display.take()
                {
                    self.viewer.update(cx, |v, cx| v.show_canvas(display, cx));
                }
            }
        }
    }
}

impl Focusable for AppView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl AppView {
    /// Hand the children what this frame shows (strip marks, change line,
    /// menus, title, width class), all from `now` and `parts`. The list's
    /// marks and the Library's open stack are handed over on change instead
    /// ([`Self::sync_left_out`], [`Self::sync_painted`], [`Self::sync_library`]).
    fn sync_children(
        &mut self,
        now: &Snapshot,
        parts: &TitleParts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_menus(now, window, cx);
        // The strip's marks follow the working version, also set on each
        // change to it ([`Self::sync_on_screen`]).
        let on_screen = self.on_screen_in(now);
        self.renders.update(cx, |r, _| r.set_on_screen(on_screen));
        let (line, base, shown) = self.compared_in(now, cx);
        self.params.update(cx, |p, _| p.set_compared(line, base, shown));
        // While a stack runs: its tile in the strip.
        let hold = self.hold_in(cx);
        self.renders.update(cx, |r, cx| r.set_hold(hold, cx));
        // Titled after what is open: the Library, or the stack's name.
        let open = match self.screen {
            Screen::Library => say!(cx, "toolbar-library-title"),
            Screen::Stacking => {
                parts.name.clone().unwrap_or_else(|| say!(cx, "toolbar-new-stack-title"))
            }
        };
        let title = say!(cx, "window-title-open", open = open);
        if *self.window_title != *title {
            window.set_window_title(&title);
            self.window_title = title.into();
        }
        self.set_chrome(self.chrome.at_width(window.viewport_size().width), cx);
        let narrow = self.narrow();
        self.renders.update(cx, |r, _| r.set_compact(narrow));
    }
}

impl AppView {
    /// Hand the Library the stack open in Stacking, while it is shown, on
    /// each change to what it is drawn from, never per frame.
    pub(crate) fn sync_library(&self, cx: &mut Context<Self>) {
        if self.screen != Screen::Library {
            return;
        }
        let now = self.snapshot(cx);
        let parts = self.title_parts_in(&now, cx);
        // The store lists a never-stacked stack only once kept, so the
        // Library lists the open one from here.
        let working = &now.working;
        let stacked = self.renders.read(cx).has_renders(&self.stack);
        let unstacked = (!stacked && !working.list.is_empty()).then(|| UnstackedNow {
            named: parts.name.is_some(),
            name: parts.name.clone().unwrap_or_else(|| say!(cx, "app-untitled-stack")),
            frames: working.list.shots().len(),
            first: working.list.files.first().cloned(),
        });
        // "Edited since #4": the working version against its base.
        let showing = now.showing();
        let edited = showing
            .base()
            .filter(|_| showing.is_edited())
            .map(|base| say!(cx, "history-edited-since", number = base.number));
        let current = CurrentStack {
            stack: Some(self.stack.clone()),
            base: self.base.clone(),
            edited,
            unstacked,
        };
        let details = self.chrome.inspector_chosen();
        self.library.update(cx, |l, cx| l.set_current(current, details, cx));
    }

    /// Hand the list the frames the result left out, with why
    /// ([`StackingEvent::Shown`]).
    fn sync_left_out(&self, cx: &mut Context<Self>) {
        let words = crate::words::of(cx);
        let left_out: HashMap<PathBuf, String> = self
            .stacking
            .read(cx)
            .result()
            .into_iter()
            .flat_map(|stack| stack.left_out_frames())
            .filter_map(|(path, why)| {
                let said = deptheos_app::left_out_message(words, [(path, why)])?;
                Some((path.to_path_buf(), said))
            })
            .collect();
        self.files.update(cx, |f, cx| f.set_left_out(left_out, cx));
    }

    /// In Retouch, hand the list the frames painted from.
    fn sync_painted(&self, cx: &mut Context<Self>) {
        let paint = self.paint.read(cx);
        let painted = self.is_retouching().then(|| {
            let frames = paint.painted_from().iter().filter_map(|k| match k {
                deptheos_app::paint::SourceKind::Frame { path } => Some(path.clone()),
                _ => None,
            });
            frames.collect()
        });
        self.files.update(cx, |f, cx| f.set_painted(painted, cx));
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The zoom field follows the zoom while not edited. The viewer
        // settles zoom while it paints, so look each frame, not on notify.
        self.sync_zoom_input(window, cx);
        // Read once for the frame: syncing the children changes none of it.
        let now = self.snapshot(cx);
        let parts = self.title_parts_in(&now, cx);
        self.sync_children(&now, &parts, window, cx);
        let library = self.screen == Screen::Library;
        // ⌘. cancels whatever runs (`can_cancel` says whether it can).
        let running = !matches!(self.running, Running::Idle);
        // Only the screen shown is built.
        let screen = match library {
            true => self.render_library_screen(window, cx),
            false => self.render_stacking_screen(&now, window, cx),
        };
        let toolbar = self.render_toolbar(parts, cx);
        let painting = self.is_retouching();
        // Undo and Redo exist only while paint mode has a step to take.
        let [can_undo, can_redo] = self.paint.read(cx).steps().map(|n| n > 0);
        let root = div()
            .id("deptheos-root")
            .key_context(if library && self.can_cancel(cx) {
                LIBRARY_RUNNING_KEY_CONTEXT
            } else if library {
                LIBRARY_KEY_CONTEXT
            } else if painting {
                PAINT_KEY_CONTEXT
            } else if running {
                STACKING_KEY_CONTEXT
            } else {
                KEY_CONTEXT
            })
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, window, cx| {
                    this.alt_tap(event, window, cx);
                    // Option (Alt) during a drag on the rail steps it.
                    this.rail_modifiers(event.modifiers.alt, window, cx);
                },
            ))
            .capture_key_down(cx.listener(|this, _: &gpui::KeyDownEvent, _, _| this.alt_spoiled()))
            .on_key_up(cx.listener(|this, event: &gpui::KeyUpEvent, _, cx| {
                if event.keystroke.key == "space" {
                    this.viewer.update(cx, |v, _| v.set_space_pan(false));
                }
                if event.keystroke.key == "\\" {
                    this.peek_source(false, cx);
                }
            }));
        self.bind_actions(root, can_undo, can_redo, cx)
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<SidebarResize>, window, cx| {
                this.resize_sidebar(event.event.position.x, window, cx)
            }))
            // Every move of a drag from outside the window.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                if this.file_drag.is_none() {
                    let paths = event.drag(cx).paths().to_vec();
                    this.set_file_drag(list::FileDrag::of(paths), cx);
                }
            }))
            .on_file_drop_exit(cx.listener(|this, _, _, cx| {
                this.set_file_drag(list::FileDrag::None, cx);
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.drop_files(paths, window, cx)
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_tap_guard(cx))
            .child(toolbar)
            .child(screen)
            .when(!self.file_drag.is_none(), |el| el.child(self.render_drop_overlay(cx)))
    }
}

impl AppView {
    /// The Library, with the welcome over it on the first run.
    fn render_library_screen(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // The first run: nothing to list yet.
        let welcome = self.is_empty(cx).then(|| self.render_welcome(window, cx));
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(self.library.clone())
            .children(welcome)
            .into_any_element()
    }

    /// Stacking: the sidebar, the photo, the strip and the inspector.
    fn render_stacking_screen(
        &mut self,
        now: &Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let retouch_labels = self.render_retouch_labels(cx);
        let progress_card = self.render_progress_card(cx);
        let welcome = self.is_empty(cx).then(|| self.render_welcome(window, cx));
        let sidebar = self.render_sidebar(cx);
        let view_bar = self.render_view_bar(cx);
        let bar_shown = welcome.is_none() && self.view_bar_shown(cx);
        self.viewer.update(cx, |v, _| v.set_fit_bottom_inset(if bar_shown { 58.0 } else { 0.0 }));
        let info_over_photo = self.render_info_over_photo(cx);
        let inspector = self.render_inspector(now, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .relative()
            .when(self.chrome.shows_sidebar(), |el| {
                el.child(
                    div()
                        .relative()
                        .h_full()
                        .flex_shrink_0()
                        .w(self.sidebar_width)
                        .child(sidebar)
                        .child(self.render_sidebar_handle(cx)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .flex()
                            .flex_col()
                            .child(self.viewer.clone())
                            .children(retouch_labels)
                            .children(welcome)
                            .children(info_over_photo)
                            // A run's card over the bar; the bar shows
                            // under it only while the run is the stack on
                            // screen's, and spans the photo at most.
                            .when(bar_shown || progress_card.is_some(), |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .bottom_3()
                                        .left_3()
                                        .right_3()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .gap_2()
                                        .children(progress_card)
                                        .when(bar_shown, |el| {
                                            el.child(
                                                div()
                                                    .w_full()
                                                    .flex()
                                                    .justify_center()
                                                    .child(view_bar),
                                            )
                                        }),
                                )
                            }),
                    )
                    .when(self.strip_shown(cx), |el| el.child(self.renders.clone())),
            )
            .when(self.chrome.shows_inspector(true), |el| {
                let panel = div()
                    .debug_selector(|| "inspector".into())
                    .h_full()
                    .flex_shrink_0()
                    .w(px(INSPECTOR_WIDTH));
                if self.cramped() {
                    // Over the photo and the strip.
                    el.child(
                        panel.absolute().top_0().right_0().occlude().shadow_lg().child(inspector),
                    )
                } else {
                    el.child(panel.child(inspector))
                }
            })
            .into_any_element()
    }
}

/// The engine's loads as the viewer's [`Loader`].
fn engine_loader(engine: Arc<Engine>) -> Loader {
    Arc::new(move |source: &Source| match source {
        // RAW/DNG previews use the file's own color science to match the
        // result: LibRaw for camera RAW (converted DNGs are only for stacking).
        Source::File(path) => engine.load_preview(path, deptheos_app::RawBackend::LibRaw),
        Source::Compare { path, frame_index, stack, overlay } => {
            engine.load_compare(path, *frame_index, deptheos_app::Look::stored(stack), *overlay)
        }
        Source::Render { stack, other } => {
            let other = other()?;
            let look = deptheos_app::Look::shown(&other, &deptheos_app::Adjustments::NEUTRAL);
            engine.load_render_compare(stack, look)
        }
    })
}
