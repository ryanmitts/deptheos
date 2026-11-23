//! The file list: the images the next Stack is made from
//! ([`deptheos_app::frames::list::List`]), one row per shot (a RAW and its
//! JPEG are one row). Rows are addressed by [`FileId`], stable across
//! changes, in natural order of folder then name or capture time
//! ([`FileList::set_order`]); thumbnails come from [`crate::thumbs`].
//!
//! The *shown* row is the one in the viewer; the *selection* (Finder rules)
//! is what actions apply to and always holds it; an *excluded* row keeps
//! its place, dimmed. A selection of two or more rows is stacked alone.
//!
//! Every change emits [`FileListEvent::Changed`], for the app to save it.

use std::collections::{HashMap, HashSet};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deptheos_app::frames::folder::{Shot, Source};
use deptheos_app::frames::list::{self, List as ImageList, StackProblem};
use deptheos_app::frames::names::file_label;
use deptheos_app::library::history::Reach;
use deptheos_app::{Frames, Moves, RawBackend};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EventEmitter, Modifiers, MouseButton, ObjectFit, SharedString,
    Subscription, Task, WeakEntity, Window, div, img, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, h_flex, v_flex,
};

use crate::app::actions::ClearFrames;
use crate::multi_select::{Click, MultiSelect};
use crate::thumbs::{Thumb, Thumbs};
use crate::r#type::TypeStyled as _;
use crate::words::{DesktopWords, Wording as _, say};
use deptheos_app::FrameOrder;

/// A row's identity, stable for the life of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId(u64);

impl FileId {
    /// `count` distinct ids, for tests that only carry them.
    #[cfg(test)]
    pub fn for_tests(count: u64) -> Vec<Self> {
        (0..count).map(Self).collect()
    }
}

struct FileRow {
    id: FileId,
    /// The shot this row shows: its files share a name.
    shot: Shot,
    /// The shot's file the row stands for, which a stack reads.
    path: PathBuf,
    included: bool,
    /// Whether the file is readable: it may be missing or offline.
    reach: Reach,
}

impl FileRow {
    /// Missing or offline: it stays excluded, and Stack waits for it.
    fn is_gone(&self) -> bool {
        self.reach != Reach::There
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileListEvent {
    /// A row moved to or clicked, or taking the place of the shown row
    /// removed.
    Selected(FileId),
    /// The list changed: save it.
    Changed,
    /// The last row was removed.
    Cleared,
}

pub struct FileList {
    list: ImageList,
    rows: Vec<FileRow>,
    /// Row ids by shot, so a row keeps its id while the list changes.
    ids: HashMap<(PathBuf, String), FileId>,
    next_id: u64,
    /// A stack is running: nothing in the list can change.
    locked: bool,
    /// The frames the result on screen left out, with why; their rows are marked.
    left_out: HashMap<PathBuf, String>,
    /// In Retouch, the frames painted from (a dot each, no eyes); `None`
    /// outside it.
    painted: Option<Vec<PathBuf>>,
    view: Entity<ListState<FileDelegate>>,
    thumbs: Thumbs,
    /// The selected rows in selection order ([`MultiSelect`]); holds the
    /// shown row.
    selection: MultiSelect<FileId>,
    /// The modifiers of the row click the list is about to report.
    click: Option<Modifiers>,
    _list_events: Subscription,
    _thumb_results: Task<()>,
    /// The off-thread file check ([`Self::check_files`]).
    _file_check: Task<()>,
    /// The Library's roots, which say whether a file is missing or offline.
    frames: Arc<Frames>,
    /// The order check's notes ([`ImageList::order_notes`]): folders whose
    /// capture order differs, or left in name order and why.
    order_notes: Vec<String>,
    _order_check: Task<()>,
    /// Sorting the list into an order off the UI thread.
    sorting: Sorting,
    /// The words the summary and the menus are in.
    words: DesktopWords,
}

impl EventEmitter<FileListEvent> for FileList {}

/// A sort that reads capture times, off the UI thread; others sort in place.
///
/// `Idle` → `Running` when such an order is chosen ([`FileList::set_order`])
/// or files are added or relinked under it. `Running` → `Running` when a
/// later request replaces it (its task dropped) or the list changed while
/// it read (sorted again). `Running` → `Idle` when shown, dropped while
/// locked, replaced by an in-place order, or by [`FileList::set_list`].
enum Sorting {
    /// The list is in its order.
    Idle,
    /// Sorting into `order`, which the list takes when the result lands.
    Running { order: deptheos_app::FrameOrder, _task: Task<()> },
}

impl FileList {
    /// A list whose files' reach is asked of `frames` ([`Frames::reach`]).
    pub fn new(frames: Arc<Frames>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = FileDelegate { files: cx.weak_entity() };
        let view = cx.new(|cx| ListState::new(delegate, window, cx));
        let _list_events =
            cx.subscribe_in(&view, window, |this: &mut Self, _, event: &ListEvent, window, cx| {
                let (ix, modifiers) = match event {
                    ListEvent::Confirm(ix) => (ix, this.click.take().unwrap_or_default()),
                    ListEvent::Select(ix) => {
                        this.click = None;
                        (ix, Modifiers::default())
                    }
                    _ => return,
                };
                let Some(id) = this.rows.get(ix.row).map(|r| r.id) else { return };
                match this.click_row(id, modifiers) {
                    Some(shown) => {
                        if shown != id {
                            this.highlight(Some(shown), window, cx);
                        }
                        cx.emit(FileListEvent::Selected(shown));
                    }
                    // ⌘ took out the last row selected: none is.
                    None => this.highlight(None, window, cx),
                }
                this.changed(cx);
            });
        let mut thumbs = Thumbs::default();
        let mut made = thumbs.receive().expect("a new Thumbs has its receiver");
        // Thumbnails arrive in bursts: take what is there, redraw once.
        let _thumb_results = cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            while let Some(first) = made.next().await {
                let mut batch = vec![first];
                while let Ok(more) = made.try_recv() {
                    batch.push(more);
                }
                let kept = this.update(cx, |this, cx| {
                    let kept = batch.into_iter().fold(false, |k, m| this.thumbs.insert(m) | k);
                    if kept {
                        this.changed(cx);
                    }
                });
                if kept.is_err() {
                    break;
                }
            }
        });
        Self {
            list: ImageList::default(),
            rows: Vec::new(),
            ids: HashMap::new(),
            next_id: 0,
            locked: false,
            left_out: HashMap::new(),
            painted: None,
            view,
            thumbs,
            selection: MultiSelect::none(),
            click: None,
            _list_events,
            _thumb_results,
            _file_check: Task::ready(()),
            frames,
            order_notes: Vec::new(),
            _order_check: Task::ready(()),
            sorting: Sorting::Idle,
            words: cx.wording().clone(),
        }
    }

    pub fn order(&self) -> deptheos_app::FrameOrder {
        self.list.order
    }

    /// See [`Self::order_notes`]'s field.
    pub fn order_notes(&self) -> &[String] {
        &self.order_notes
    }

    /// Sort each folder's frames by name or capture time; refused while
    /// locked. The last choice wins, even over a sort in flight to the
    /// current order ([`Self::sort_into`]).
    pub fn set_order(
        &mut self,
        order: deptheos_app::FrameOrder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() || self.wanted_order() == order {
            return;
        }
        self.sort_into(order, window, cx);
    }

    /// The order the list is in, or is being sorted into.
    fn wanted_order(&self) -> deptheos_app::FrameOrder {
        match &self.sorting {
            Sorting::Running { order, .. } => *order,
            Sorting::Idle => self.list.order,
        }
    }

    /// Sort into `order`, replacing a sort in flight. An off-thread result
    /// applies only while its list is still shown: one edited meanwhile is
    /// sorted again, one replaced by [`Self::set_list`] drops it.
    fn sort_into(
        &mut self,
        order: deptheos_app::FrameOrder,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from = self.list.clone();
        let mut list = from.clone();
        list.order = order;
        if !list.sorts_from_disk() {
            list.sort();
            self.set_sorting(Sorting::Idle);
            if list != self.list {
                self.show_list(list, window, cx);
                cx.emit(FileListEvent::Changed);
            }
            return;
        }
        let task = cx.spawn_in(window, async move |this, cx| {
            let sorted = crate::work::interactive("sort", move || {
                list.sort();
                list
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                let Ok(sorted) = sorted else {
                    this.set_sorting(Sorting::Idle);
                    return;
                };
                if !this.editable() {
                    tracing::debug!("the list can't change: the sort is dropped");
                    this.set_sorting(Sorting::Idle);
                    return;
                }
                if this.list != from {
                    tracing::debug!("the list changed while it sorted: sorting it again");
                    this.sort_into(order, window, cx);
                    return;
                }
                this.set_sorting(Sorting::Idle);
                this.show_list(sorted, window, cx);
                cx.emit(FileListEvent::Changed);
            })
            .ok();
        });
        self.set_sorting(Sorting::Running { order, _task: task });
    }

    /// The files changed: sort again if sorting, or if the order reads
    /// capture times (adds and relinks only place files near their spot).
    fn sort_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let order = self.wanted_order();
        if matches!(self.sorting, Sorting::Running { .. }) || self.list.sorts_from_disk() {
            self.sort_into(order, window, cx);
        }
    }

    fn set_sorting(&mut self, sorting: Sorting) {
        match &sorting {
            Sorting::Idle if matches!(self.sorting, Sorting::Idle) => {}
            Sorting::Idle => tracing::debug!("list sort: idle"),
            Sorting::Running { order, .. } => tracing::debug!("list sort: sorting into {order:?}"),
        }
        self.sorting = sorting;
    }

    /// Whether a sort is in flight: the list isn't in its order yet.
    pub fn sorting(&self) -> bool {
        matches!(self.sorting, Sorting::Running { .. })
    }

    /// Check the frame order again, off the UI thread.
    fn check_order(&mut self, cx: &mut Context<Self>) {
        let list = self.list.clone();
        let words = crate::words::of(cx).clone();
        self._order_check = cx.spawn(async move |this, cx| {
            let Ok(notes) =
                crate::work::interactive("order-check", move || list.order_notes(&words)).await
            else {
                return;
            };
            this.update(cx, |this, cx| {
                this.order_notes = notes;
                cx.notify();
            })
            .ok();
        });
    }

    // ------------------------------------------------------------ the list

    pub fn list(&self) -> &ImageList {
        &self.list
    }

    /// Replace the list (a render's, or last time's), showing nothing and
    /// emitting nothing: the caller shows what goes with it. Drops a sort in
    /// flight; refused while locked.
    pub fn set_list(&mut self, list: ImageList, window: &mut Window, cx: &mut Context<Self>) {
        if self.locked {
            return;
        }
        self.set_sorting(Sorting::Idle);
        self.show_list(list, window, cx);
    }

    /// Show `list`, keeping a sort in flight.
    fn show_list(&mut self, list: ImageList, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.selected(cx);
        self.list = list;
        self.rebuild();
        self.rows_moved(shown, window, cx);
        self.check_files(cx);
    }

    /// Add the images and folders picked, expanded by [`ImageList::expand`]
    /// off the UI thread (network shares and cards read slowly). In capture
    /// order rows go after their folder's, then into place when the times
    /// come back ([`Self::sort_into`]). Returns the rows added; refused while
    /// locked.
    pub fn add(
        &mut self,
        files: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<FileId> {
        if !self.editable() {
            return Vec::new();
        }
        let before: HashSet<FileId> = self.rows.iter().map(|r| r.id).collect();
        if self.list.add_files_unsorted(files) == 0 {
            return Vec::new();
        }
        let shown = self.selected(cx);
        // The new files were just listed from disk: they are there.
        self.rebuild();
        self.rows_moved(shown, window, cx);
        cx.emit(FileListEvent::Changed);
        self.sort_again(window, cx);
        if !self.sorting() {
            // A sort's result checks the order when it is shown.
            self.check_order(cx);
        }
        self.rows.iter().map(|r| r.id).filter(|id| !before.contains(id)).collect()
    }

    /// Which file of a RAW + JPEG pair the rows stand for.
    pub fn source(&self) -> Source {
        self.list.source
    }

    /// Whether any shot has both a RAW and a JPEG, so the source matters.
    pub fn has_pairs(&self) -> bool {
        self.rows.iter().any(|r| r.shot.is_pair())
    }

    /// Point every row at its shot's file of `source`; a shown row whose file
    /// changed is shown again ([`FileListEvent::Selected`]).
    pub fn set_source(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() || self.list.source == source {
            return;
        }
        let shown = self.selected(cx);
        let before = shown.and_then(|id| self.path(id).map(Path::to_path_buf));
        self.list.source = source;
        self.rebuild();
        self.rows_moved(shown, window, cx);
        self.check_files(cx);
        cx.emit(FileListEvent::Changed);
        if let Some(id) = shown.filter(|&id| self.path(id) != before.as_deref()) {
            cx.emit(FileListEvent::Selected(id));
        }
    }

    /// Rows from the list, keeping each shot's id, without reading the disk:
    /// a new row's file counts as there until [`Self::check_files`].
    fn rebuild(&mut self) {
        let reach: HashMap<FileId, Reach> = self.rows.iter().map(|r| (r.id, r.reach)).collect();
        let mut rows = Vec::new();
        for shot in self.list.shots() {
            let folder = shot.files[0].parent().map(Path::to_path_buf).unwrap_or_default();
            let key = (folder, shot.stem.to_lowercase());
            let id = match self.ids.get(&key) {
                Some(&id) => id,
                None => {
                    let id = FileId(self.next_id);
                    self.next_id += 1;
                    self.ids.insert(key, id);
                    id
                }
            };
            let path = shot.file(self.list.source).to_path_buf();
            let reach = reach.get(&id).copied().unwrap_or(Reach::There);
            let included = self.list.is_included(&shot) && reach == Reach::There;
            rows.push(FileRow { id, shot, path, included, reach });
        }
        self.rows = rows;
        let paths: Vec<PathBuf> =
            self.rows.iter().filter(|r| !r.is_gone()).map(|r| r.path.clone()).collect();
        self.thumbs.request(paths);
    }

    /// Check each row's file off the UI thread (shares and cards are slow)
    /// via [`Frames::reach`], and mark the rows; a later check replaces this.
    fn check_files(&mut self, cx: &mut Context<Self>) {
        self.check_order(cx);
        let (ids, paths): (Vec<FileId>, Vec<PathBuf>) =
            self.rows.iter().map(|r| (r.id, r.path.clone())).unzip();
        let frames = self.frames.clone();
        self._file_check = cx.spawn(async move |this, cx| {
            let checked = crate::work::interactive("file-check", move || {
                let reach = frames.reach(&paths);
                ids.into_iter().zip(paths).zip(reach).map(|((id, p), r)| (id, p, r)).collect()
            });
            let Ok(checked) = checked.await else { return };
            let _ = this.update(cx, |this, cx| this.mark_reach(checked, cx));
        });
    }

    /// Mark rows as [`Self::check_files`] found them; a row whose file
    /// changed since waits for the next check.
    fn mark_reach(&mut self, checked: Vec<(FileId, PathBuf, Reach)>, cx: &mut Context<Self>) {
        let checked: HashMap<FileId, (PathBuf, Reach)> =
            checked.into_iter().map(|(id, path, reach)| (id, (path, reach))).collect();
        let mut changed = false;
        for row in &mut self.rows {
            if let Some((path, reach)) = checked.get(&row.id)
                && *path == row.path
                && *reach != row.reach
            {
                row.reach = *reach;
                row.included = self.list.is_included(&row.shot) && !row.is_gone();
                changed = true;
            }
        }
        if changed {
            let paths: Vec<PathBuf> =
                self.rows.iter().filter(|r| !r.is_gone()).map(|r| r.path.clone()).collect();
            self.thumbs.request(paths);
            cx.notify();
        }
    }

    // ----------------------------------------------------- include/exclude

    pub fn set_included(&mut self, id: FileId, included: bool, cx: &mut Context<Self>) {
        self.set_included_rows(&[id], included, cx);
    }

    /// Include or exclude `ids`; a missing file stays excluded. Refused
    /// while locked, since the running stack was started from the list as it was.
    fn set_included_rows(&mut self, ids: &[FileId], included: bool, cx: &mut Context<Self>) {
        self.apply_included(|row| ids.contains(&row.id).then_some(included), cx);
    }

    /// Apply `include` to the rows it names; if any changed, rebuild and
    /// emit [`FileListEvent::Changed`]. Refused unless editable.
    fn apply_included(
        &mut self,
        include: impl Fn(&FileRow) -> Option<bool>,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() {
            return;
        }
        let mut changed = false;
        for row in &self.rows {
            if let Some(included) = include(row)
                && self.list.is_included(&row.shot) != included
            {
                self.list.set_included(&row.shot, included);
                changed = true;
            }
        }
        if changed {
            self.rebuild();
            self.changed(cx);
            cx.emit(FileListEvent::Changed);
        }
    }

    /// Include or exclude every row.
    fn set_all_included(&mut self, included: bool, cx: &mut Context<Self>) {
        let ids: Vec<FileId> = self.rows.iter().map(|r| r.id).collect();
        self.set_included_rows(&ids, included, cx);
    }

    /// Delete or Backspace: exclude the selected rows, never remove them.
    pub fn exclude_selected(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_rows();
        self.set_included_rows(&ids, false, cx);
    }

    /// X: exclude the selection, or include it when all are excluded.
    pub fn toggle_excluded(&mut self, cx: &mut Context<Self>) {
        let ids = self.selected_rows();
        let any_included = self.rows.iter().any(|r| ids.contains(&r.id) && r.included);
        self.set_included_rows(&ids, !any_included, cx);
    }

    pub fn set_locked(&mut self, locked: bool, cx: &mut Context<Self>) {
        self.locked = locked;
        self.changed(cx);
    }

    #[cfg(test)]
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Whether the list can change: no stack is running.
    fn editable(&self) -> bool {
        !self.locked
    }

    /// The frames the result on screen left out, with why
    /// ("Left out IMG_0412.CR3 (…)").
    pub fn set_left_out(&mut self, left_out: HashMap<PathBuf, String>, cx: &mut Context<Self>) {
        if self.left_out != left_out {
            self.left_out = left_out;
            self.changed(cx);
        }
    }

    /// The left-out frames' rows, in list order.
    pub fn left_out_rows(&self) -> Vec<FileId> {
        self.rows.iter().filter(|r| self.why_left_out(r).is_some()).map(|r| r.id).collect()
    }

    /// Why `row`'s shot was left out of the result on screen, if it was.
    fn why_left_out(&self, row: &FileRow) -> Option<&String> {
        std::iter::once(&row.path).chain(&row.shot.files).find_map(|p| self.left_out.get(p))
    }

    /// Select `ids` (in list order), the first shown, without emitting.
    pub fn select_rows(&mut self, ids: Vec<FileId>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&first) = ids.first() else { return };
        self.highlight(Some(first), window, cx);
        self.selection = MultiSelect::of(ids);
        self.changed(cx);
    }

    /// In Retouch, the frames painted from; `None` outside it.
    pub fn set_painted(&mut self, painted: Option<Vec<PathBuf>>, cx: &mut Context<Self>) {
        if self.painted != painted {
            self.painted = painted;
            self.changed(cx);
        }
    }

    /// Remove from List: take `ids` out of the list. Refused while locked.
    pub(crate) fn remove(&mut self, ids: &[FileId], window: &mut Window, cx: &mut Context<Self>) {
        if !self.editable() || ids.is_empty() {
            return;
        }
        let shots: Vec<Shot> =
            self.rows.iter().filter(|r| ids.contains(&r.id)).map(|r| r.shot.clone()).collect();
        // The shown row's place goes to the row after it, else before it.
        let shown = self.selected(cx);
        let next = shown.filter(|id| ids.contains(id)).and_then(|_| {
            let at = self.rows.iter().position(|r| ids.contains(&r.id))?;
            let after = self.rows[at..].iter().find(|r| !ids.contains(&r.id));
            after.or_else(|| self.rows[..at].iter().next_back()).map(|r| r.id)
        });
        self.list.remove(&shots);
        if self.list.is_empty() {
            self.clear(window, cx);
            return;
        }
        self.rebuild();
        let keep = if shown.is_some_and(|id| ids.contains(&id)) { next } else { shown };
        self.rows_moved(keep, window, cx);
        self.check_order(cx);
        cx.emit(FileListEvent::Changed);
        if let Some(next) = next {
            cx.emit(FileListEvent::Selected(next));
        }
    }

    pub fn can_clear(&self) -> bool {
        self.editable() && !self.rows.is_empty()
    }

    /// Clear List: empty it ([`FileListEvent::Cleared`]).
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_clear() {
            return;
        }
        self.list.clear();
        self.rebuild();
        self.rows_moved(None, window, cx);
        self.check_order(cx);
        cx.emit(FileListEvent::Changed);
        cx.emit(FileListEvent::Cleared);
    }

    /// Include Only These: include the selected rows and exclude the rest.
    pub fn include_only_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_rows();
        self.include_only(&selected, cx);
    }

    /// Include `selected` and exclude every other row.
    fn include_only(&mut self, selected: &[FileId], cx: &mut Context<Self>) {
        self.apply_included(|row| Some(selected.contains(&row.id)), cx);
    }

    /// Include every frame.
    pub fn include_all(&mut self, cx: &mut Context<Self>) {
        self.apply_included(|_| Some(true), cx);
    }

    /// Select every row, keeping the shown one shown.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = MultiSelect::of(self.rows.iter().map(|r| r.id).collect());
        self.changed(cx);
    }

    /// Whether every frame is included.
    pub fn all_included(&self) -> bool {
        self.rows.iter().all(|r| r.included)
    }

    /// Whether X excludes (else every selected row is excluded and it includes).
    pub fn selection_excludes(&self) -> bool {
        let ids = self.selected_rows();
        self.rows.iter().any(|r| ids.contains(&r.id) && r.included)
    }

    // ------------------------------------------------------------- missing

    /// Remove the rows whose files are missing or offline.
    pub fn remove_missing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<FileId> = self.rows.iter().filter(|r| r.is_gone()).map(|r| r.id).collect();
        self.remove(&ids, window, cx);
    }

    /// How many files can't be read, missing or not connected.
    pub fn missing_count(&self) -> usize {
        self.rows.iter().filter(|r| r.is_gone()).count()
    }

    /// How many of [`Self::missing_count`] are on a volume not connected.
    pub fn not_connected_count(&self) -> usize {
        self.rows.iter().filter(|r| r.reach == Reach::NotConnected).count()
    }

    /// The first file that can't be read: what Locate… asks to find.
    pub fn first_missing(&self) -> Option<&Path> {
        self.rows.iter().find(|r| r.is_gone()).map(|r| r.path.as_path())
    }

    /// Folders were relinked ([`Moves`]): rows keep their ids, so the shown
    /// row and selection stay; capture order sorts again ([`Self::sort_into`]).
    pub fn follow(&mut self, moves: &Moves, window: &mut Window, cx: &mut Context<Self>) {
        let moved = self.list.follow(moves);
        if moved.is_empty() {
            return;
        }
        let shown = self.selected(cx);
        self.move_ids(&moved);
        self.rebuild();
        self.rows_moved(shown, window, cx);
        self.check_files(cx);
        cx.emit(FileListEvent::Changed);
        if self.editable() {
            self.sort_again(window, cx);
        }
    }

    /// Re-id each shot moved in `moved`, counting it there until the next
    /// check ([`ImageList::follow`]).
    fn move_ids(&mut self, moved: &HashMap<PathBuf, PathBuf>) {
        let key = |path: &Path| {
            let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase());
            (folder, stem.unwrap_or_default())
        };
        let mut found = HashSet::new();
        for (from, to) in moved {
            if let Some(id) = self.ids.remove(&key(from)) {
                found.insert(*self.ids.entry(key(to)).or_insert(id));
            }
        }
        for row in &mut self.rows {
            if found.contains(&row.id) {
                row.reach = Reach::There;
            }
        }
    }

    // --------------------------------------------------------------- rows

    /// The highlight is a row index, so reset it from the file id after rows
    /// move; drops thumbnails of files no longer listed.
    fn rows_moved(&mut self, shown: Option<FileId>, window: &mut Window, cx: &mut Context<Self>) {
        let listed: HashSet<FileId> = self.rows.iter().map(|r| r.id).collect();
        self.selection.retain(|id| listed.contains(id));
        let paths: HashSet<PathBuf> = self.rows.iter().map(|r| r.path.clone()).collect();
        self.thumbs.retain(|p| paths.contains(p), window, cx);
        self.view.update(cx, |list, cx| list.set_right_clicked_index(None, window, cx));
        self.highlight(shown, window, cx);
        self.changed(cx);
    }

    fn position(&self, id: FileId) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id)
    }

    fn row(&self, id: FileId) -> Option<&FileRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.row(id).map(|r| r.path.as_path())
    }

    /// Every file of the shot `id` shows (a RAW and its JPEG).
    pub fn shot_files(&self, id: FileId) -> Option<&[PathBuf]> {
        self.row(id).map(|r| r.shot.files.as_slice())
    }

    /// The row whose shot has the file at `path`.
    pub fn row_of(&self, path: &Path) -> Option<FileId> {
        self.rows.iter().find(|r| r.path == path || r.shot.contains(path)).map(|r| r.id)
    }

    /// Every row: its id and file, in list order.
    pub fn rows(&self) -> impl Iterator<Item = (FileId, PathBuf)> + '_ {
        self.rows.iter().map(|r| (r.id, r.path.clone()))
    }

    /// Any non-DNG camera RAW: the frames the RAW backend choice applies to.
    pub fn has_camera_raw(&self) -> bool {
        self.rows.iter().any(|r| is_convertible_raw(&r.path))
    }

    /// The included rows, in list order.
    pub fn included(&self) -> Vec<(FileId, PathBuf)> {
        self.rows.iter().filter(|r| r.included).map(|r| (r.id, r.path.clone())).collect()
    }

    /// What Stack is given: every file in order (recorded as the render's
    /// list) and the included ones, which it stacks. The selection plays no
    /// part; Include Only These turns it into the stack.
    pub fn stack_input(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        // Both files of a pair, so clicking the render brings the pair back.
        let listed = self.rows.iter().flat_map(|r| r.shot.files.iter().cloned()).collect();
        let stacked = self.rows.iter().filter(|r| r.included).map(|r| r.path.clone()).collect();
        (listed, stacked)
    }

    /// The frames Stack runs on, in list order.
    #[cfg(test)]
    pub fn stack_frames(&self) -> Vec<PathBuf> {
        self.stack_input().1
    }

    /// Included rows whose file is gone: Stack can't run with them.
    fn missing_included(&self) -> usize {
        self.rows.iter().filter(|r| r.is_gone() && self.list.is_included(&r.shot)).count()
    }

    /// Why Stack can't run on the included frames ([`list::stack_problem`]).
    /// Both RAW decoders decode linear, so the choice doesn't matter here.
    pub fn stack_problem(&self) -> Option<StackProblem> {
        let included = self.stack_input().1;
        list::stack_problem(self.rows.len(), &included, self.missing_included(), RawBackend::LibRaw)
    }

    /// No [`Self::stack_problem`] and no sort in flight (frames just added in
    /// capture order aren't placed yet).
    pub fn can_stack(&self) -> bool {
        self.stack_problem().is_none() && !self.sorting()
    }

    /// "12 frames", or "10 of 12 frames" with two excluded.
    #[cfg(test)]
    pub fn summary(&self) -> SharedString {
        let (included, total) = self.counts();
        frames_summary(&self.words, included, total).into()
    }

    /// The included rows, and all of them. A missing file isn't included.
    pub fn counts(&self) -> (usize, usize) {
        (self.rows.iter().filter(|r| r.included).count(), self.rows.len())
    }

    /// The highlighted (shown) row; [`Self::selected_rows`] is the selection.
    pub fn selected(&self, cx: &App) -> Option<FileId> {
        let ix = self.view.read(cx).selected_index()?;
        self.rows.get(ix.row).map(|r| r.id)
    }

    /// Highlight a row without emitting [`FileListEvent::Selected`]; a row
    /// outside the selection becomes the whole selection.
    pub fn highlight(&mut self, id: Option<FileId>, window: &mut Window, cx: &mut Context<Self>) {
        let at = id.and_then(|id| self.position(id));
        let id = id.filter(|_| at.is_some());
        match id {
            Some(id) if self.selection.contains(&id) => {}
            Some(id) => self.selection = MultiSelect::one(id),
            None => self.selection = MultiSelect::none(),
        }
        let ix = at.map(|row| IndexPath::default().row(row));
        self.view.update(cx, |list, cx| list.set_selected_index(ix, window, cx));
    }

    // ------------------------------------------------------------ selection

    /// A click on `id` by the Finder's rules ([`MultiSelect`]). Returns the
    /// row to show: the one clicked, or after ⌘ removed it the last left.
    fn click_row(&mut self, id: FileId, modifiers: Modifiers) -> Option<FileId> {
        self.position(id)?;
        let order: Vec<FileId> = self.rows.iter().map(|r| r.id).collect();
        self.selection.click(&id, Click::of(modifiers), &order);
        match self.selection.contains(&id) {
            true => Some(id),
            false => self.selection.last().copied(),
        }
    }

    /// The selected rows, in list order.
    pub fn selected_rows(&self) -> Vec<FileId> {
        self.selection.in_order(self.rows.iter().map(|r| &r.id))
    }

    /// A right-click's rows: the selection if `id` is in it, else `id`.
    fn targets(&self, id: FileId) -> Vec<FileId> {
        if self.selection.contains(&id) { self.selected_rows() } else { vec![id] }
    }

    /// Give the rows the keyboard, as a click on one does.
    #[cfg(test)]
    pub fn focus_rows(&self, window: &mut Window, cx: &mut App) {
        let handle = gpui::Focusable::focus_handle(self.view.read(cx), cx);
        window.focus(&handle, cx);
    }

    /// Rows changed: this entity's observers and the list both redraw.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.view.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    /// The included rows below the last target, for Exclude Frames After This.
    fn included_after(&self, targets: &[FileId]) -> Vec<FileId> {
        let Some(last) = self.rows.iter().rposition(|r| targets.contains(&r.id)) else {
            return Vec::new();
        };
        self.rows[last + 1..].iter().filter(|r| r.included).map(|r| r.id).collect()
    }

    /// What the right-click menu offers, read before building it (which
    /// needs the menu's own context).
    fn menu_model(&self, cx: &App) -> MenuModel {
        let clicked = self.view.read(cx).right_clicked_index();
        let target = clicked.and_then(|ix| self.rows.get(ix.row)).map(|r| r.id);
        let targets = target.map(|id| self.targets(id)).unwrap_or_default();
        let after = self.included_after(&targets);
        MenuModel {
            targets_included: self.rows.iter().any(|r| targets.contains(&r.id) && r.included),
            targets,
            after,
            editable: self.editable(),
            order: self.list.order,
            can_sort: self.editable() && !self.rows.is_empty(),
            any_included: self.rows.iter().any(|r| r.included),
            any_excluded: self.rows.iter().any(|r| !r.included && !r.is_gone()),
            can_clear: self.can_clear(),
            words: self.words.clone(),
        }
    }

    /// The right-click menu: row items act on the targets, the rest on every row.
    fn context_menu(m: MenuModel, menu: PopupMenu, this: WeakEntity<Self>) -> PopupMenu {
        let item = |label: SharedString, enabled: bool, action: Box<MenuAction>| {
            let this = this.clone();
            PopupMenuItem::new(label).disabled(!enabled).on_click(move |_, window, cx| {
                this.update(cx, |files, cx| action(files, window, cx)).ok();
            })
        };
        let w = &m.words;
        let mut menu = menu;
        if !m.targets.is_empty() {
            let several = m.targets.len() > 1;
            let include = !m.targets_included;
            let (toggle, remove, after) = (m.targets.clone(), m.targets.clone(), m.after.clone());
            let only = m.targets.clone();
            let label = if include { say!(w, "files-include") } else { say!(w, "files-exclude") };
            menu = menu.item(item(
                label.into(),
                m.editable,
                Box::new(move |f, _, cx| f.set_included_rows(&toggle, include, cx)),
            ));
            if several {
                menu = menu.item(item(
                    say!(w, "files-include-only").into(),
                    m.editable,
                    Box::new(move |f, _, cx| f.include_only(&only, cx)),
                ));
            }
            menu = menu
                .item(item(
                    if several {
                        say!(w, "files-exclude-after-these")
                    } else {
                        say!(w, "files-exclude-after-this")
                    }
                    .into(),
                    m.editable && !m.after.is_empty(),
                    Box::new(move |f, _, cx| f.set_included_rows(&after, false, cx)),
                ))
                .item(item(
                    if several {
                        say!(w, "files-remove-frames-from-list")
                    } else {
                        say!(w, "files-remove-from-list")
                    }
                    .into(),
                    m.editable,
                    Box::new(move |f, w, cx| f.remove(&remove, w, cx)),
                ))
                .separator();
        }
        menu.item(item(
            say!(w, "files-include-all").into(),
            m.editable && m.any_excluded,
            Box::new(|f, _, cx| f.set_all_included(true, cx)),
        ))
        .item(item(
            say!(w, "files-exclude-all").into(),
            m.editable && m.any_included,
            Box::new(|f, _, cx| f.set_all_included(false, cx)),
        ))
        .separator()
        .item(
            item(
                say!(w, "files-sort-by-name").into(),
                m.can_sort,
                Box::new(|f, w, cx| f.set_order(FrameOrder::Name, w, cx)),
            )
            .checked(m.order == FrameOrder::Name),
        )
        .item(
            item(
                say!(w, "files-sort-by-capture-time").into(),
                m.can_sort,
                Box::new(|f, w, cx| f.set_order(FrameOrder::CaptureTime, w, cx)),
            )
            .checked(m.order == FrameOrder::CaptureTime),
        )
        .separator()
        .item(
            PopupMenuItem::new(say!(w, "files-remove-all-frames"))
                .disabled(!m.can_clear)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ClearFrames), cx)),
        )
    }
}

/// A context-menu command.
type MenuAction = dyn Fn(&mut FileList, &mut Window, &mut Context<FileList>);

/// What the right-click menu offers ([`FileList::menu_model`]).
struct MenuModel {
    /// The row items' rows; none below the rows.
    targets: Vec<FileId>,
    /// Some of the targets are included: the menu offers Exclude.
    targets_included: bool,
    /// What Exclude Frames After This excludes.
    after: Vec<FileId>,
    /// The list can change ([`FileList::editable`]).
    editable: bool,
    order: deptheos_app::FrameOrder,
    can_sort: bool,
    any_included: bool,
    any_excluded: bool,
    can_clear: bool,
    words: DesktopWords,
}

impl Render for FileList {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.view.clone();
        let files = cx.weak_entity();
        let rows = div()
            .id("file-list")
            .flex_1()
            .min_w_0()
            .h_full()
            // Rows set the right-click target as the press bubbles up; clear
            // it on capture so a press below the rows doesn't reuse the last.
            .capture_any_mouse_down(move |event, window, cx| {
                if event.button == MouseButton::Right {
                    view.update(cx, |list, cx| list.set_right_clicked_index(None, window, cx));
                }
            })
            // The kit sizes its scrollbar from rows it has seen: empty, it
            // would keep the last list's length.
            .child(List::new(&self.view).scrollbar_visible(!self.rows.is_empty()).size_full())
            .context_menu(move |menu, _, cx| match files.upgrade() {
                Some(this) => {
                    let model = this.read(cx).menu_model(cx);
                    FileList::context_menu(model, menu, files.clone())
                }
                None => menu,
            });
        v_flex().flex_1().min_h(px(120.0)).child(rows)
    }
}

/// Non-DNG camera RAW: what the Adobe backend converts, so what the RAW
/// decoder choice applies to.
pub(crate) fn is_convertible_raw(path: &Path) -> bool {
    RawBackend::AdobeDng.converts(path)
}

/// The frame number's column in a row, in points: room for three digits.
const FRAME_NUMBER_WIDTH: f32 = 22.0;

/// A thumbnail's box in a row, in points: 3:2, like most camera frames.
const THUMB_BOX: (f32, f32) = (48.0, 32.0);

/// A row's thumbnail in [`THUMB_BOX`]: blank while made, the extension when
/// there is none.
fn render_thumb(
    thumb: Option<&Thumb>,
    path: &Path,
    missing: bool,
    cx: &App,
) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let frame = div()
        .w(px(THUMB_BOX.0))
        .h(px(THUMB_BOX.1))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .overflow_hidden();
    if missing {
        return frame
            .bg(theme.muted)
            .child(Icon::new(IconName::TriangleAlert).small().text_color(theme.warning));
    }
    match thumb {
        Some(Thumb::Ready(image)) => {
            frame.child(img(image.clone()).size_full().object_fit(ObjectFit::Contain))
        }
        Some(Thumb::Pending { .. }) => frame.bg(theme.muted),
        Some(Thumb::Failed) | None => {
            let ext = path.extension().map(|e| e.to_string_lossy().to_uppercase());
            frame
                .bg(theme.muted)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(ext.unwrap_or_default()))
        }
    }
}

/// Draws [`FileList`]'s rows; holds no rows itself.
struct FileDelegate {
    files: WeakEntity<FileList>,
}

impl ListDelegate for FileDelegate {
    type Item = ListItem;

    fn items_count(&self, _section: usize, cx: &App) -> usize {
        self.files.upgrade().map_or(0, |files| files.read(cx).rows.len())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let files = self.files.upgrade()?;
        let number = ix.row + 1;
        let row = {
            let files = files.read(cx);
            let row = files.rows.get(ix.row)?;
            let left_out = files.why_left_out(row).cloned().map(SharedString::from);
            let name = SharedString::from(file_label(&row.path));
            let folder: SharedString =
                row.path.parent().map(|p| p.display().to_string()).unwrap_or_default().into();
            let thumb = render_thumb(files.thumbs.get(&row.path), &row.path, row.is_gone(), cx);
            let selected = files.selection.contains(&row.id);
            let (included, missing) = (row.included, row.is_gone());
            let unplugged = row.reach == Reach::NotConnected;
            // In Retouch: whether this frame is painted from.
            let painted = files.painted.as_ref().map(|p| p.contains(&row.path));
            let locked = files.locked;
            let row = (row.id, name, folder, included, missing, selected, thumb, locked, painted);
            (row, left_out, unplugged)
        };
        let (
            (id, name, folder, included, missing, selected, thumb, locked, painted),
            left_out,
            unplugged,
        ) = row;
        let weak = self.files.clone();
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        // The list draws the hover; the selection tint is the shared one.
        let selected_bg = crate::look::selected_bg(cx);
        // The select event has no modifiers: the row's click handler records
        // them first.
        let click = weak.clone();
        let item = ListItem::new(("file", id.0))
            .group("file-row")
            .on_click(move |event, _, cx| {
                let modifiers = event.modifiers();
                click.update(cx, |files, _| files.click = Some(modifiers)).ok();
            })
            // The list tints the shown row; the rest of the selection here.
            .when(selected, |item| item.bg(selected_bg))
            .when(missing && !selected, |item| item.bg(warning.opacity(0.12)));
        let eye = Button::new(("include", id.0))
            .icon(if included { IconName::Eye } else { IconName::EyeOff })
            .xsmall()
            .ghost()
            .disabled(missing || locked)
            .tooltip(if unplugged {
                say!(cx, "files-eye-not-connected")
            } else if missing {
                say!(cx, "files-eye-missing")
            } else if locked {
                say!(cx, "files-eye-locked")
            } else if included {
                say!(cx, "files-eye-exclude")
            } else {
                say!(cx, "files-eye-include")
            })
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                weak.update(cx, |files, cx| files.set_included(id, !included, cx)).ok();
            });
        let eye = match painted {
            Some(painted) => div()
                .flex_shrink_0()
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .when(painted, |el| {
                    el.child(div().size(px(6.0)).rounded_full().bg(cx.theme().foreground))
                }),
            // Included rows show the eye on hover or selection, so excluded
            // ones stand out.
            _ => div()
                .flex_shrink_0()
                .when(!included, |el| el.opacity(0.6))
                .when(included && !selected, |el| {
                    el.invisible().group_hover("file-row", |s| s.visible())
                })
                .child(eye),
        };
        Some(
            item.child(
                h_flex()
                    .id(("file-row", id.0))
                    .gap_2()
                    .w_full()
                    // The whole name, which the row may cut, over its folder.
                    .tooltip({
                        let name = name.clone();
                        move |window, cx| {
                            let (name, folder) = (name.clone(), folder.clone());
                            gpui_kit::component::tooltip::Tooltip::element(move |_, cx| {
                                v_flex().child(name.clone()).child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(folder.clone()),
                                )
                            })
                            .build(window, cx)
                        }
                    })
                    .child(
                        div()
                            .w(px(FRAME_NUMBER_WIDTH))
                            .flex_shrink_0()
                            .text_right()
                            .text_xs()
                            .tabular()
                            .text_color(muted)
                            .child(format!("{number}")),
                    )
                    .child(div().when(!included && !missing, |el| el.opacity(0.4)).child(thumb))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(!included, |el| el.text_color(muted).line_through())
                            .when(missing, |el| el.text_color(warning))
                            .child(name),
                    )
                    .when(missing, |el| {
                        el.child(div().text_xs().text_color(warning).child(if unplugged {
                            say!(cx, "files-not-connected")
                        } else {
                            say!(cx, "files-missing")
                        }))
                    })
                    // Left out of the result on screen: a mark, why on hover.
                    .when_some(left_out.filter(|_| !missing), |el, why| {
                        el.child(
                            div()
                                .id(("left-out", id.0))
                                .debug_selector(|| "left-out-mark".into())
                                .flex_shrink_0()
                                .child(
                                    Icon::new(IconName::TriangleAlert).xsmall().text_color(warning),
                                )
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(why.clone())
                                        .build(window, cx)
                                }),
                        )
                    })
                    .child(eye),
            ),
        )
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .px_4()
            .child(say!(cx, "files-empty-title"))
            .child(div().text_xs().text_center().child(say!(cx, "files-empty-text")))
    }

    fn set_selected_index(
        &mut self,
        _ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        // Nothing to keep: selections reach FileList as `ListEvent`s.
    }
}

/// "12 frames", "10 of 12 frames": how many a list stacks, of how many.
#[cfg(test)]
pub(crate) fn frames_summary(w: &DesktopWords, included: usize, total: usize) -> String {
    match total {
        0 => say!(w, "files-summary-none"),
        total if included == total => say!(w, "frames-count", n = total),
        total => say!(w, "files-summary-some", included = included, total = total),
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;
