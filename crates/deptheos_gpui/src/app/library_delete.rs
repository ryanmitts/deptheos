//! Delete Library (Settings ▸ Library): whether it can be deleted, deleting
//! it, and the main window left as at first launch. Settings asks first and
//! calls in through [`MainWindow`]; the delete runs on the Library's worker
//! ([`crate::renders::Renders::delete_library`]).

use gpui::{AnyWindowHandle, App, Context, Entity, Global, WeakEntity, Window};

use std::path::PathBuf;

use super::{AppView, Screen};
use crate::notices::{Kind, Subject};
use crate::prefs::prefs;
use crate::words::{DesktopWords, Wording as _, say};

/// The main window and its view, for Settings to query and delete the
/// Library, and the Library folder named when it opened. Set once the main
/// window opens.
pub(crate) struct MainWindow {
    window: AnyWindowHandle,
    view: WeakEntity<AppView>,
    chosen: Option<PathBuf>,
}

impl Global for MainWindow {}

/// Note the main window, for [`deletable`] and [`delete_library`].
pub(crate) fn register(window: AnyWindowHandle, view: &Entity<AppView>, cx: &mut App) {
    let chosen = prefs(cx).history_root();
    cx.set_global(MainWindow { window, view: view.downgrade(), chosen });
}

/// The main window's view, while it is open.
pub(crate) fn main_view(cx: &App) -> Option<Entity<AppView>> {
    cx.try_global::<MainWindow>().and_then(|main| main.view.upgrade())
}

/// Whether the whole Library can be deleted now, and if not, why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Deletable {
    /// The Library folder couldn't be opened (or there is no main window).
    Unopened,
    /// Settings names another folder than `open`, used at the next start.
    Moved { open: PathBuf },
    /// The Library is still being read at launch.
    Listing,
    /// Nothing is in it.
    Empty,
    /// A stack, batch or sequence is running.
    Running,
    /// A render, result or export is still being written.
    Writing,
    /// A delete is running.
    Deleting,
    /// It can be: what it holds.
    Ready { stacks: usize, renders: usize },
}

impl Deletable {
    /// Why it can't be deleted now; none when it can.
    pub(crate) fn reason(&self, w: &DesktopWords) -> Option<String> {
        Some(match self {
            Deletable::Unopened => say!(w, "prefs-delete-library-unopened"),
            Deletable::Moved { open } => {
                say!(w, "prefs-delete-library-moved", folder = open.display().to_string())
            }
            Deletable::Listing => say!(w, "prefs-delete-library-listing"),
            Deletable::Empty => say!(w, "prefs-delete-library-empty"),
            Deletable::Running => say!(w, "prefs-delete-library-running"),
            Deletable::Writing => say!(w, "prefs-delete-library-writing"),
            Deletable::Deleting => say!(w, "prefs-delete-library-deleting"),
            Deletable::Ready { .. } => return None,
        })
    }
}

/// Whether the Library can be deleted now ([`AppView::deletable`]).
pub(crate) fn deletable(cx: &App) -> Deletable {
    main_view(cx).map_or(Deletable::Unopened, |view| view.read(cx).deletable(cx))
}

/// Delete the whole Library, in the main window (Settings has asked).
pub(crate) fn delete_library(cx: &mut App) {
    let Some(main) = cx.try_global::<MainWindow>() else { return };
    let (window, view) = (main.window, main.view.clone());
    let deleted = window
        .update(cx, |_, window, cx| view.update(cx, |view, cx| view.delete_library(window, cx)));
    if !matches!(deleted, Ok(Ok(()))) {
        tracing::warn!("the main window is gone: the Library is not deleted");
    }
}

impl AppView {
    /// The Library can be deleted: Settings names the open folder, nothing
    /// runs or is written, and it isn't empty (Recently Deleted included).
    pub(crate) fn deletable(&self, cx: &App) -> Deletable {
        let renders = self.renders.read(cx);
        let store = renders.store();
        let chosen = cx.try_global::<MainWindow>().map(|main| &main.chosen);
        if chosen.is_some_and(|chosen| *chosen != prefs(cx).history_root()) {
            return Deletable::Moved { open: store.root().to_path_buf() };
        }
        if renders.listing().is_listing() {
            return Deletable::Listing;
        }
        if renders.is_deleting() {
            return Deletable::Deleting;
        }
        if self.is_processing(cx) {
            return Deletable::Running;
        }
        if self.files_being_written(cx) {
            return Deletable::Writing;
        }
        // Recently Deleted alone is something to delete too.
        let trash = renders.trash().map_or(0, |(records, _)| records);
        match (self.library.read(cx).stack_count(cx), renders.renders().len()) {
            (0, 0) if trash == 0 => Deletable::Empty,
            (stacks, renders) => Deletable::Ready { stacks, renders },
        }
    }

    /// Delete the whole Library ([`Self::library_deleted`] follows); one
    /// asked as something started is refused, saying why.
    fn delete_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.deletable(cx) {
            Deletable::Ready { .. } => {
                self.renders.update(cx, |r, cx| r.delete_library(window, cx));
            }
            // Nothing to delete.
            Deletable::Empty | Deletable::Unopened => {}
            moved @ Deletable::Moved { .. } => {
                let why = moved.reason(cx.wording()).unwrap_or_default();
                self.post(Kind::Error, Subject::Alone, why, Vec::new(), window, cx);
            }
            Deletable::Listing | Deletable::Running | Deletable::Writing | Deletable::Deleting => {
                let why = say!(cx, "library-delete-not-now");
                self.post(Kind::Error, Subject::Alone, why, Vec::new(), window, cx);
            }
        }
    }

    /// The Library is gone: the window is as at first launch, blank Library
    /// and empty stack (settings kept), nothing open.
    pub(super) fn library_deleted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_paint(cx);
        self.viewer.update(cx, |v, cx| v.clear_compare(cx));
        self.forget_result(window, cx);
        let settings = self.working(cx).settings;
        self.edit(deptheos_app::library::stacks::Working::new(settings), window, cx);
        self.set_screen(Screen::Library, window, cx);
        let open = self.stack.clone();
        self.library.update(cx, |l, cx| {
            l.close_preview(cx);
            l.select_open(open, cx);
        });
        let done = deptheos_app::library::history::library_deleted(crate::words::of(cx));
        self.post(Kind::Done, Subject::Alone, done, Vec::new(), window, cx);
        cx.notify();
    }
}
