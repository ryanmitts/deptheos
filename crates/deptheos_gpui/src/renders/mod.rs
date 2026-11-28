//! The renders: every render made ([`deptheos_app::library::history`]),
//! held for the window. The strip in Stacking shows the open stack's,
//! newest first; the Library screen (`library`) shows them all.
//!
//! Clicking a render shows its result ([`Renders::take_opened`]) and leaves
//! the working version as it is (ADR-142); [`Renders::set_on_screen`] lets a
//! tile mark the base Edited, offer Revert to #n and list Render Info. A
//! failed batch stack opens to be fixed and stacked again
//! ([`RendersEvent::Retry`]). Reads run on the pool, writes on the store
//! thread ([`crate::work`], ADR-143).
//!
//! Submodules: `queue` (store work), `opening`, `tiles`, `menus`, `info`
//! (Render Info), `labels` and `library` (the Library screen).

#[cfg(test)]
mod app_tests;
mod info;
mod labels;
mod library;
mod menus;
mod opening;
mod queue;
mod thumbnails;
mod tiles;

pub use library::{CurrentStack, Filter, Library, LibraryEvent, Step, UnstackedNow};
pub(crate) use menus::{reveal, reveal_label, show_frames_label};
pub(crate) use tiles::{render_line, render_title, thumb_box};

#[cfg(test)]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use deptheos_app::library::history::{
    self, Failure, History, Render as Saved, RenderId, StackId, StoreError,
};
use deptheos_app::library::stacks::{self, Compared, Showing, StackInfo, Working, stack_name};
use deptheos_app::{CompletedStack, CpuImage, Engine, PresentedStack};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EventEmitter, FontWeight, RenderImage, ScrollHandle, SharedString,
    Subscription, Task, Window, div, px,
};
use gpui_kit::component::input::InputState;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};

use crate::AppIcon;
use crate::stacking::Stacking;
use crate::viewer::OtherStack;
use crate::words::{DesktopWords, Wording as _, say};
use crate::work;
use deptheos_app::library::thumbnails::PER_READ;
use queue::SaveRequest;

use thumbnails::Thumbnails;

/// A stack's or render's name, or "Untitled stack" when it has none.
pub(crate) fn untitled(name: Option<String>, w: &DesktopWords) -> String {
    name.unwrap_or_else(|| say!(w, "app-untitled-stack"))
}

/// Recently Deleted's record count and bytes; none if unreadable. Runs on a worker.
fn trash_of(store: &History) -> Option<(usize, u64)> {
    match store.deleted() {
        Ok(items) => Some((items.len(), items.iter().map(|i| i.size).sum())),
        Err(e) => {
            tracing::warn!("could not read Recently Deleted: {e}");
            None
        }
    }
}

/// A strip thumbnail's height; its width follows the aspect within
/// [`THUMB_WIDTH`].
const THUMB_HEIGHT: f32 = 60.0;
const THUMB_WIDTH: (f32, f32) = (60.0, 120.0);
/// The gap between tiles.
const TILE_GAP: f32 = 2.0;
/// A tile's padding and the thumbnail's selection border, each side.
const TILE_PADDING: f32 = 4.0;
const THUMB_BORDER: f32 = 2.0;
/// The decode height: twice [`THUMB_HEIGHT`], for Retina.
const THUMB_DECODE_HEIGHT: u32 = 168;
/// How long an open runs before its tile shows a spinner, so quick ones
/// don't flash one.
const SPINNER_DELAY: Duration = Duration::from_millis(150);

pub enum RendersEvent {
    /// A render finished loading; it is waiting in [`Renders::take_opened`].
    Opened,
    /// Compare the shown result with this render (⌘-click, or the menu).
    Compare(RenderId),
    /// A finished stack was saved as this render.
    Saved {
        stack: Arc<CompletedStack>,
        render: RenderId,
        /// The base its working version had when the stack started.
        from: Option<RenderId>,
    },
    /// Stacks deleted whole, after a [`Self::Deleted`] for their renders.
    StacksDeleted(Vec<StackId>),
    /// The whole Library was deleted, after a [`Self::Deleted`] for its
    /// renders; the working version is empty too.
    LibraryDeleted,
    /// Records put in Recently Deleted, for a notice with Undo.
    MovedToDeleted(history::Deletion),
    /// These renders were deleted.
    Deleted {
        ids: Vec<RenderId>,
        /// The render pinned as A was among them.
        compared: bool,
    },
    /// Something failed: said in an error notice about `about`.
    Failed { message: crate::notices::Worded, about: crate::notices::Subject },
    /// A finished stack could not be saved (a [`Self::Failed`] says why).
    NotSaved { stack: Arc<CompletedStack>, message: SharedString },
    /// A failed batch stack clicked: load its images and settings.
    Retry(Box<Failure>),
    /// Export Grow Sequence… from a render's menu.
    ExportGrow(RenderId),
    /// Adjustments were pasted onto this render, which Adjust hadn't open.
    Pasted(RenderId),
    /// Revert to #n, from a render's menu.
    Revert(RenderId),
    /// Folders were relinked ([`crate::app::AppView::relinked`]).
    Relinked(deptheos_app::Moves),
    /// Locate… relinked a folder to `folder`, `matched` frames found by
    /// name, size and fingerprint.
    Located { folder: std::path::PathBuf, matched: deptheos_app::frames::sources::Matched },
}

/// The working version on screen, as the app view last said; the strip
/// reads it for Edited, Revert to #n and Render Info.
#[derive(Clone, Debug)]
pub struct OnScreen {
    pub working: Working,
    /// What counts as a change on the desktop, for its list.
    pub compared: Compared,
    /// Retouch is open: Revert to #n is off until it closes.
    pub retouching: bool,
}

/// An opened render, ready to show.
pub struct OpenedRender {
    pub render: Saved,
    pub presented: PresentedStack,
}

/// Opening a render.
///
/// Lifecycle: `Idle` or `Loading` a render, changed only in
/// `renders/opening.rs`. A newer open replaces the one in flight; leaving
/// `Loading` drops its tasks; a load applies only while its `generation`
/// is current.
///
/// ```text
/// Idle ──open──▶ Loading ──loaded, failed or canceled──▶ Idle
///                                 └──(SPINNER_DELAY)──▶ Loading { slow }
/// ```
pub(super) enum Open {
    Idle,
    Loading {
        id: RenderId,
        generation: u64,
        /// It has taken long enough to show a spinner.
        slow: bool,
        _load: Task<()>,
        _spinner: Task<()>,
    },
}

/// Reading the Library's renders, once at launch.
///
/// Lifecycle: `Listing` (holding the read), `Listed` or `Failed`, changed
/// only by [`Renders::set_listing`]. Saves wait until it leaves `Listing`.
///
/// ```text
/// Listing ──the listing came in──▶ Listed
///    └──────it couldn't be read──▶ Failed (said in a notice)
/// ```
pub enum LibraryRead {
    Listing { _read: Task<()> },
    Listed,
    Failed,
}

impl LibraryRead {
    fn name(&self) -> &'static str {
        match self {
            Self::Listing { .. } => "listing",
            Self::Listed => "listed",
            Self::Failed => "failed",
        }
    }

    /// Whether the read is still running.
    pub fn is_listing(&self) -> bool {
        matches!(self, Self::Listing { .. })
    }
}

impl std::fmt::Debug for LibraryRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// What runs over the strip.
///
/// Lifecycle: `Free` (it opens renders), `Locked` (a stack, sequence, batch
/// or stack switch runs) or `Running` (a stack into the stack on screen,
/// with its tile), any to any, changed only by [`Renders::set_hold`], from
/// the app's `hold_in` (`app/run.rs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hold {
    Free,
    Locked,
    Running { number: u32, progress: f32 },
}

impl Hold {
    fn name(&self) -> &'static str {
        match self {
            Hold::Free => "free",
            Hold::Locked => "locked",
            Hold::Running { .. } => "running",
        }
    }
}

/// The strip's one Library write at a time, so a save never lands inside a
/// delete, restore or Empty Recently Deleted.
///
/// Lifecycle: `Idle`, `Saving` or `Deleting`, each holding its task;
/// changed only by [`Renders::set_writer`] (`renders/queue.rs`). Requests
/// meanwhile wait in the queue ([`Renders::pump`]).
///
/// ```text
/// Idle ──the queue's next save──▶ Saving ──it landed──▶ Idle
/// Idle ──a delete, the queue's next restore or Empty──▶ Deleting
/// Deleting ──it landed──▶ Idle, or ──it stopped part way──▶ Deleting (rereading)
/// ```
pub(super) enum Writer {
    Idle,
    Saving { _task: Task<()> },
    Deleting { _task: Task<()> },
}

impl Writer {
    fn saving(task: Task<()>) -> Self {
        Self::Saving { _task: task }
    }

    fn deleting(task: Task<()>) -> Self {
        Self::Deleting { _task: task }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Saving { .. } => "saving",
            Self::Deleting { .. } => "deleting",
        }
    }
}

/// A title being edited.
struct Renaming {
    render: RenderId,
    input: Entity<InputState>,
    _events: Subscription,
}

pub struct Renders {
    engine: Arc<Engine>,
    /// The Library: the window never runs without it.
    store: Arc<History>,
    /// Oldest first.
    renders: Vec<Saved>,
    /// Each render's bytes on disk as last read ([`Self::refresh_stacks`]);
    /// none for one saved since.
    sizes: HashMap<RenderId, u64>,
    /// Batch stacks that failed, oldest first.
    failures: Vec<Failure>,
    /// The stacks with a name or a kept working version.
    stacks: HashMap<StackId, StackInfo>,
    /// The [`Self::stacks`] read in flight; a newer one replaces it.
    stacks_read: Option<Task<()>>,
    /// Each render's thumbnail, read as it is wanted in view.
    thumbnails: Thumbnails,
    /// The render each strip child shows, as last drawn (none for a divider
    /// or failure), matched to the scroll handle's child bounds.
    strip_ids: Vec<Option<RenderId>>,
    /// Holds the result on screen and its render; only read here.
    stacking: Entity<Stacking>,
    /// What [`Self::stacking`] showed when last seen, to notice a change.
    seen: (Option<usize>, Option<RenderId>),
    /// The open in flight, if any.
    open: Open,
    /// The render pinned as the compare reference.
    compare: Option<RenderId>,
    /// Set by [`Self::set_hold`].
    hold: Hold,
    /// The working version on screen ([`Self::set_on_screen`]).
    on_screen: Option<OnScreen>,
    /// Render Info.
    ///
    /// Lifecycle: `None` (closed) or `Some` render, changed only by
    /// [`Renders::set_info`] (`renders/info.rs`). Opened from a tile's menu,
    /// Stack ▸ Render Info or ⌘I; closed by a click outside, Esc, its render
    /// deleted, its stack off screen or the Library shown.
    info: Option<RenderId>,
    /// A narrow window: tiles drop their settings line.
    pub(crate) compact: bool,
    renaming: Option<Renaming>,
    listing: LibraryRead,
    saves: VecDeque<SaveRequest>,
    writer: Writer,
    /// The last open's number: each open takes the next.
    generation: u64,
    /// The render an open finished loading, until [`Self::take_opened`].
    opened: Option<OpenedRender>,
    /// Recently Deleted's record count and bytes, as last read.
    trash: Option<(usize, u64)>,
    /// Counts landed deletes, restores and Empties, so the Library rereads
    /// Recently Deleted on change.
    trash_changes: u64,
    scroll: ScrollHandle,
    _shown: Subscription,
}

impl EventEmitter<RendersEvent> for Renders {}

impl Renders {
    /// Show the renders in `store`, listing them in the background.
    pub fn new(
        engine: Arc<Engine>,
        store: Arc<History>,
        stacking: Entity<Stacking>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = cx.wording().clone();
        let list_failed = say!(cx, "renders-list-failed");
        let listing = {
            let store = store.clone();
            cx.spawn_in(window, async move |this, cx| {
                let listed = work::interactive("library-list", {
                    let store = store.clone();
                    move || {
                        Ok::<_, StoreError>((
                            store.list()?,
                            store.failures()?,
                            store.stacks().unwrap_or_default(),
                            trash_of(&store),
                        ))
                    }
                })
                .await;
                let listed = crate::notices::worked(&words, &list_failed, listed);
                this.update_in(cx, |this, window, cx| {
                    match listed {
                        Ok((listed, failures, stacks, trash)) => {
                            this.add_listed(listed);
                            this.failures = failures;
                            this.stacks = stacks;
                            this.trash = trash;
                            // The sizes walk every render's folder: after
                            // the grid's first paint.
                            this.refresh_stacks(cx);
                        }
                        Err(why) => {
                            this.fail(why, cx);
                            this.set_listing(LibraryRead::Failed, cx);
                        }
                    }
                    if this.listing.is_listing() {
                        this.set_listing(LibraryRead::Listed, cx);
                        // Look for moved roots before anything is written.
                        this.saves.push_front(SaveRequest::FindAgain);
                    }
                    this.pump(window, cx);
                    cx.notify();
                    // The newest thumbnails; the rest as they scroll in.
                    let ids = this.renders.iter().rev().take(PER_READ);
                    let ids: Vec<RenderId> = ids.map(|r| r.id.clone()).collect();
                    this.request_thumbnails(ids, cx);
                })
                .ok();
            })
        };
        cx.observe_global::<crate::color::Screen>(|this, cx| {
            this.thumbnails.follow(crate::color::shown(), cx);
            cx.notify();
        })
        .detach();
        // Redraw when the result or its render changes, not every tick.
        let _shown = cx.observe(&stacking, |this, stacking, cx| {
            let stacking = stacking.read(cx);
            let now = (
                stacking.result().map(|s| Arc::as_ptr(s) as usize),
                stacking.shown_render().cloned(),
            );
            if now != this.seen {
                this.seen = now;
                cx.notify();
            }
        });
        Self {
            engine,
            store,
            renders: Vec::new(),
            sizes: HashMap::new(),
            failures: Vec::new(),
            stacks: HashMap::new(),
            stacks_read: None,
            thumbnails: Thumbnails::default(),
            strip_ids: Vec::new(),
            stacking,
            seen: (None, None),
            open: Open::Idle,
            compare: None,
            hold: Hold::Free,
            on_screen: None,
            info: None,
            compact: false,
            renaming: None,
            listing: LibraryRead::Listing { _read: listing },
            saves: VecDeque::new(),
            writer: Writer::Idle,
            generation: 0,
            opened: None,
            trash: None,
            trash_changes: 0,
            scroll: ScrollHandle::new(),
            _shown,
        }
    }

    pub fn store(&self) -> &Arc<History> {
        &self.store
    }

    /// Whether the history's read at launch is over, listed or failed.
    #[cfg(test)]
    pub fn is_listed(&self) -> bool {
        !self.listing.is_listing()
    }

    /// Where the read of the history at launch is.
    pub fn listing(&self) -> &LibraryRead {
        &self.listing
    }

    /// The only way [`Self::listing`] changes.
    pub(crate) fn set_listing(&mut self, listing: LibraryRead, cx: &mut Context<Self>) {
        tracing::debug!(from = self.listing.name(), to = listing.name(), "library read");
        self.listing = listing;
        cx.notify();
    }

    /// Every render, oldest first.
    pub fn renders(&self) -> &[Saved] {
        &self.renders
    }

    /// Whether `stack` has a render: stacked at least once.
    pub fn has_renders(&self, stack: &StackId) -> bool {
        self.renders.iter().any(|r| r.stack == *stack)
    }

    pub fn find(&self, id: &RenderId) -> Option<&Saved> {
        self.renders.iter().find(|r| r.id == *id)
    }

    pub fn is_saving(&self) -> bool {
        matches!(self.writer, Writer::Saving { .. }) || self.saves.iter().any(|s| !s.deletes())
    }

    /// The render on screen.
    pub fn active<'a>(&self, cx: &'a App) -> Option<&'a RenderId> {
        self.stacking.read(cx).shown_render()
    }

    pub fn is_active(&self, id: &RenderId, cx: &App) -> bool {
        self.active(cx) == Some(id)
    }

    /// A copy of the app view's working version, set on every change to it
    /// (via `save_list`), as Retouch opens and closes, and as it draws.
    /// Readers run only after those changes, so it is never behind.
    pub fn set_on_screen(&mut self, on_screen: OnScreen) {
        self.on_screen = Some(on_screen);
    }

    /// The working version against `render`, for Revert to #n and Render
    /// Info; `None` for another stack's, or before the app view has said.
    pub(crate) fn showing_for<'a>(&'a self, render: &'a Saved) -> Option<Showing<'a>> {
        let on = self.on_screen.as_ref()?;
        if on.working.stack != render.stack {
            return None;
        }
        let base = on.working.base.as_ref().and_then(|b| self.find(b));
        Some(Showing::new(&on.working, Some(render), base, on.compared))
    }

    /// A narrow window: tiles drop their settings line.
    pub fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
    }

    pub fn is_locked(&self) -> bool {
        !matches!(self.hold, Hold::Free)
    }

    /// The one place [`Hold`] changes; locking cancels the open in flight.
    pub fn set_hold(&mut self, hold: Hold, cx: &mut Context<Self>) {
        if hold == self.hold {
            return;
        }
        tracing::debug!(from = self.hold.name(), to = hold.name(), "strip hold");
        let locking = matches!(self.hold, Hold::Free);
        self.hold = hold;
        if locking {
            self.cancel_open();
        }
        cx.notify();
    }

    /// Nothing on screen: no failure loaded, no render compared or opening.
    pub fn clear_shown(&mut self, cx: &mut Context<Self>) {
        self.compare = None;
        self.cancel_open();
        cx.notify();
    }

    /// The render pinned as the compare reference.
    pub fn compare(&self) -> Option<&RenderId> {
        self.compare.as_ref()
    }

    pub fn set_compare(&mut self, render: RenderId, cx: &mut Context<Self>) {
        self.compare = Some(render);
        cx.notify();
    }

    pub fn clear_compare(&mut self, cx: &mut Context<Self>) {
        if self.compare.take().is_some() {
            cx.notify();
        }
    }

    /// A render's label for the viewer and toolbar: its title and settings.
    pub fn label(&self, id: &RenderId, w: &DesktopWords) -> Option<SharedString> {
        let render = self.find(id)?;
        let (title, settings) = (self.title(render, w), render.config.summary(w.shared()));
        Some(say!(w, "render-label", title = title, settings = settings).into())
    }

    /// Its given name, else its folder's, else "Untitled stack".
    pub fn stack_name(&self, stack: &StackId, w: &DesktopWords) -> String {
        untitled(self.given_stack_name(stack), w)
    }

    /// The name a stack was given, else its folder's; `None` with neither.
    pub fn given_stack_name(&self, stack: &StackId) -> Option<String> {
        stack_name(stack, self.stacks.get(stack), &self.renders)
    }

    /// Every rendered stack's [`Self::stack_name`], in one pass.
    pub fn stack_names(&self, w: &DesktopWords) -> HashMap<StackId, String> {
        // Each stack's lowest-numbered render, which its name comes from.
        let mut first: HashMap<&StackId, &Saved> = HashMap::new();
        for r in &self.renders {
            let lowest = first.entry(&r.stack).or_insert(r);
            if r.number < lowest.number {
                *lowest = r;
            }
        }
        first
            .into_iter()
            .map(|(stack, r)| {
                let name = stack_name(stack, self.stacks.get(stack), std::slice::from_ref(r));
                (stack.clone(), untitled(name, w))
            })
            .collect()
    }

    /// What the store keeps of a stack beside its renders.
    pub fn stack_info(&self, stack: &StackId) -> Option<&StackInfo> {
        self.stacks.get(stack)
    }

    /// Stacks with frames kept and no render ("Not stacked yet").
    pub fn unstacked(&self) -> Vec<(&StackId, &StackInfo)> {
        deptheos_app::library::stacks::unstacked(&self.stacks, &self.renders)
    }

    /// `ids`' bytes on disk, "610 MB"; `None` until all are measured.
    pub fn size_text<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a RenderId>,
        w: &DesktopWords,
    ) -> Option<String> {
        let mut total = 0;
        for id in ids {
            total += self.sizes.get(id)?;
        }
        Some(deptheos_app::words::units::size_text(total, w.shared()))
    }

    /// What Delete Library frees, "1.2 GB", Recently Deleted included;
    /// `None` until all is measured.
    pub fn library_size_text(&self, w: &DesktopWords) -> Option<String> {
        let mut total = self.trash?.1;
        for render in &self.renders {
            total += self.sizes.get(&render.id)?;
        }
        Some(deptheos_app::words::units::size_text(total, w.shared()))
    }

    /// Delete Other Renders: `id`'s stack less `id` and the starred.
    pub fn others_of(&self, id: &RenderId) -> Vec<RenderId> {
        let Some(keep) = self.find(id) else { return Vec::new() };
        stacks::others_of(keep, &self.renders).into_iter().map(|r| r.id.clone()).collect()
    }

    /// "canon1 #4", or "canon1 #4 · Final" when named; an only render is
    /// numbered too, as its export is.
    pub fn title(&self, render: &Saved, w: &DesktopWords) -> String {
        let stack = self.stack_name(&render.stack, w);
        let name = history::render_name(w.shared(), render.number, render.name.as_deref(), false);
        say!(w, "render-title", stack = stack, render = name)
    }

    /// Recently Deleted's record count and bytes, as last read.
    pub fn trash(&self) -> Option<(usize, u64)> {
        self.trash
    }

    /// Counts the changes to Recently Deleted landed so far.
    pub(crate) fn trash_changes(&self) -> u64 {
        self.trash_changes
    }

    /// Recently Deleted's records, newest first, read in the background.
    pub(crate) fn read_deleted(
        &self,
    ) -> impl Future<Output = Result<Vec<history::DeletedItem>, String>> + use<> {
        let store = self.store.clone();
        work::try_run(
            work::Lane::Background,
            work::Key::kind("library-deleted"),
            work::root(),
            move || store.deleted(),
        )
    }

    /// Recently Deleted's thumbnails for `ids` at strip size, read in the
    /// background; none where missing (failed stacks) or unreadable.
    pub(crate) fn read_deleted_thumbnails(
        &self,
        ids: Vec<history::DeletedId>,
    ) -> impl Future<Output = Vec<(history::DeletedId, Option<CpuImage>)>> + use<> {
        let store = self.store.clone();
        let read = work::background("library-deleted-thumbnails", move || {
            ids.into_iter()
                .map(|id| match store.deleted_thumbnail(&id) {
                    Ok(image) => (id, Some(queue::strip_thumbnail(&image))),
                    Err(e) => {
                        tracing::debug!("no thumbnail for deleted {}: {e}", id.as_str());
                        (id, None)
                    }
                })
                .collect::<Vec<_>>()
        });
        async move { read.await.unwrap_or_default() }
    }

    /// Reread stack names and kept working versions (switching stacks
    /// writes them).
    pub fn refresh_stacks(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        self.stacks_read = Some(cx.spawn(async move |this, cx| {
            let read = work::try_run(
                work::Lane::Interactive,
                work::Key::kind("library-stacks"),
                work::root(),
                move || {
                    store.stacks().map(|stacks| (stacks, store.render_sizes(), trash_of(&store)))
                },
            )
            .await;
            match read {
                Ok((stacks, sizes, trash)) => {
                    this.update(cx, |this, cx| {
                        this.stacks = stacks;
                        this.sizes = sizes;
                        this.trash = trash;
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => tracing::warn!("could not read the stacks: {e}"),
            }
        }));
    }

    /// Name a stack, or with an empty name revert to its folder's. A failed
    /// save restores the old name unless renamed again meanwhile.
    pub fn rename_stack(&mut self, stack: &StackId, name: &str, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let name = Some(name.trim()).filter(|n| !n.is_empty()).map(String::from);
        let old = std::mem::replace(
            &mut self.stacks.entry(stack.clone()).or_default().name,
            name.clone(),
        );
        cx.notify();
        let stack = stack.clone();
        let words = cx.wording().clone();
        let lead = say!(cx, "render-stack-rename-failed");
        cx.spawn(async move |this, cx| {
            let written = name.clone();
            let renamed = work::store({
                let stack = stack.clone();
                move || store.rename_stack(&stack, written.as_deref())
            })
            .await;
            if let Err(why) = crate::notices::worked(&words, &lead, renamed) {
                this.update(cx, |this, cx| {
                    if let Some(info) = this.stacks.get_mut(&stack)
                        && info.name == name
                    {
                        info.name = old;
                    }
                    this.fail(why, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The strip's renders: the stack on screen's, newest first, by index.
    fn this_stack(&self) -> Vec<usize> {
        let stack = self.on_screen.as_ref().map(|o| &o.working.stack);
        let mut ours: Vec<usize> =
            (0..self.renders.len()).filter(|&i| Some(&self.renders[i].stack) == stack).collect();
        ours.sort_by_key(|&i| std::cmp::Reverse(self.renders[i].number));
        ours
    }

    /// The strip's renders, as their ids.
    #[cfg(test)]
    pub fn strip(&self) -> Vec<RenderId> {
        self.this_stack().into_iter().map(|i| self.renders[i].id.clone()).collect()
    }

    /// A render's thumbnail, once read.
    pub fn thumbnail(&self, id: &RenderId) -> Option<Arc<RenderImage>> {
        self.thumbnails.get(id)
    }

    /// Read `id`'s stored picture (up to
    /// [`deptheos_app::library::history::THUMBNAIL_SIDE`] px) for a large
    /// look; not kept.
    pub fn read_picture(&self, id: &RenderId) -> impl Future<Output = Option<CpuImage>> + use<> {
        let store = self.store.clone();
        let id = id.clone();
        let key = work::Key::of("library-picture", &id);
        let read = work::run(work::Lane::Interactive, key, work::root(), move || {
            match store.load_thumbnail(&id) {
                Ok(image) => Some(CpuImage::from_rgb(&image)),
                Err(e) => {
                    tracing::warn!("no picture for render {}: {e}", id.as_str());
                    None
                }
            }
        });
        async move { read.await.ok().flatten() }
    }

    /// `ids` wanted in view, most wanted first; reads what the thumbnail
    /// policy says, one read at a time.
    pub fn request_thumbnails(&mut self, ids: Vec<RenderId>, cx: &mut Context<Self>) {
        if let Some(read) = self.thumbnails.want(ids, cx) {
            self.read_thumbnails(read, cx);
        }
    }

    /// Draw render `id`'s thumbnail again under its look, from `stack`
    /// with the paint `flattened` answers, then read it again where shown.
    /// A Library write, never canceled: the wait is detached.
    pub fn rewrite_thumbnail(
        &mut self,
        id: RenderId,
        stack: Arc<CompletedStack>,
        flattened: Option<std::sync::mpsc::Receiver<deptheos_app::paint::Flattened>>,
        cx: &mut Context<Self>,
    ) {
        let rewrite =
            self.store.rewrite_thumbnail(self.engine.clone(), id.clone(), stack, flattened);
        cx.spawn(async move |this, cx| match rewrite.await {
            Ok(deptheos_app::library::thumbnails::rewrite::Rewrote::Written) => {
                this.update(cx, |this, cx| this.thumbnail_rewritten(&id, cx)).ok();
            }
            Ok(deptheos_app::library::thumbnails::rewrite::Rewrote::Overtaken) => {}
            Err(e) => tracing::warn!("render {}'s thumbnail not drawn again: {e}", id.as_str()),
        })
        .detach();
    }

    /// Copy Adjustments from render `id`, which Adjust hasn't open: its
    /// sliders read after its pending writes, off this thread.
    pub fn copy_adjustments(&mut self, id: RenderId, cx: &mut Context<Self>) {
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            let read = work::background("adjust-copy", move || store.adjustments_to_export(&id));
            match read.await {
                Ok(Ok(saved)) => {
                    cx.update(|cx| crate::adjust::copy(saved.adjustments, cx));
                }
                Ok(Err(e)) => tracing::warn!("adjustments not copied: {e}"),
                Err(e) => tracing::warn!("adjustments not copied: {e}"),
            }
        })
        .detach();
    }

    /// Paste Adjustments onto render `id` (of `stack`), which Adjust
    /// hasn't open: its crop kept (ADR-153), written as a change quit waits
    /// for, then its thumbnail drawn again. [`RendersEvent::Pasted`]
    /// follows, for the window to show it again.
    pub fn paste_adjustments(
        &mut self,
        id: RenderId,
        stack: Arc<CompletedStack>,
        copied: deptheos_app::Adjustments,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let words = cx.wording().clone();
        let failed = say!(cx, "adjust-paste-failed");
        cx.spawn(async move |this, cx| {
            let onto = id.clone();
            let pasted =
                work::write("adjust-paste", move || store.paste_adjustments(&onto, &copied)).await;
            let pasted = crate::notices::worked(&words, &failed, pasted);
            this.update(cx, |this, cx| match pasted {
                Ok(_) => {
                    this.rewrite_thumbnail(id.clone(), stack, None, cx);
                    cx.emit(RendersEvent::Pasted(id));
                }
                Err(why) => this.fail(why, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Render `id`'s thumbnail was drawn again: the one held is dropped
    /// and read again while it is wanted.
    pub fn thumbnail_rewritten(&mut self, id: &RenderId, cx: &mut Context<Self>) {
        if let Some(read) = self.thumbnails.rewritten(id, cx) {
            self.read_thumbnails(read, cx);
        }
        cx.notify();
    }

    /// Start `read`, in the background.
    fn read_thumbnails(&mut self, (token, ids): thumbnails::Read, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let asked = ids.clone();
        let loading = cx.spawn(async move |this, cx| {
            let read =
                work::background("library-thumbnails", move || queue::thumbnails(&store, &ids))
                    .await
                    .unwrap_or_default();
            this.update(cx, |this, cx| {
                this.thumbnails.set_loading(None);
                if let Some(next) = this.thumbnails.read(token, asked, read, cx) {
                    this.read_thumbnails(next, cx);
                }
                cx.notify();
            })
            .ok();
        });
        self.thumbnails.set_loading(Some(loading));
    }

    /// The failed batch stack `id`, if it is one.
    pub fn failure(&self, id: &RenderId) -> Option<&Failure> {
        self.failures.iter().find(|f| f.id == *id)
    }

    /// Reads a render's stack from the store, for a viewer load.
    pub fn reader(&self, id: &RenderId) -> OtherStack {
        let store = self.store.clone();
        let id = id.clone();
        Arc::new(move || store.load_render(&id).map_err(deptheos_app::Error::from))
    }

    pub fn take_opened(&mut self) -> Option<OpenedRender> {
        self.opened.take()
    }

    /// The thumbnails read so far.
    #[cfg(test)]
    pub fn thumbnail_ids(&self) -> HashSet<RenderId> {
        self.thumbnails.shown_ids()
    }

    /// The thumbnails asked for: read, or being read.
    #[cfg(test)]
    pub fn thumbnails_requested(&self) -> HashSet<RenderId> {
        self.thumbnails.requested_ids()
    }

    /// Whether `id`'s thumbnail failed to read, last time it was asked for.
    #[cfg(test)]
    pub fn thumbnail_failed(&self, id: &RenderId) -> bool {
        self.thumbnails.failed(id)
    }

    /// Keep at most `kept` thumbnails.
    #[cfg(test)]
    pub fn set_thumbs_kept(&mut self, kept: usize) {
        self.thumbnails.set_kept(kept);
    }

    /// Let failed thumbnails be read again, as if
    /// [`deptheos_app::library::thumbnails::RETRY_AFTER`] had passed.
    #[cfg(test)]
    pub fn age_thumbnail_failures(&mut self) {
        self.thumbnails.age_failures();
    }

    /// Scroll the strip to its oldest tiles.
    #[cfg(test)]
    pub fn scroll_to_oldest(&mut self, cx: &mut Context<Self>) {
        let end = self.scroll.max_offset();
        self.scroll.set_offset(gpui::point(-end.x, px(0.0)));
        cx.notify();
    }

    /// Batch stacks that failed, oldest first.
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }
}

impl gpui::Render for Renders {
    /// The open stack's renders, newest first, after the running stack.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.close_stale_info(cx);
        let theme = cx.theme();
        let (bg, border, muted) = (theme.secondary, theme.border, theme.muted_foreground);
        let saving = self.is_saving();
        let ours = self.this_stack();
        let mut tiles: Vec<gpui::AnyElement> = Vec::new();
        // What each child shows, parallel to `tiles`.
        let mut strip_ids: Vec<Option<RenderId>> = Vec::new();
        // The base's second line while the working version is edited.
        let base_line = self.base_tile_line(cx);
        if let Hold::Running { number, progress } = self.hold {
            tiles.push(self.render_running_tile(number, progress, cx).into_any_element());
            strip_ids.push(None);
        }
        for &i in &ours {
            let render = &self.renders[i];
            strip_ids.push(Some(render.id.clone()));
            tiles.push(self.render_tile(render, base_line.as_ref(), cx));
        }
        self.load_visible(strip_ids, window, cx);
        let count = if ours.is_empty() {
            say!(cx, "renders-strip-none")
        } else {
            say!(cx, "renders-count", n = ours.len())
        };
        // Over the tiles, not beside them: the strip's width is the tiles'.
        let heading = h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .pt_1p5()
            .whitespace_nowrap()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(Icon::new(AppIcon::Render).small())
                    .child(say!(cx, "renders-strip-heading")),
            )
            .child(div().text_xs().text_color(muted).child(count))
            .when(saving, |el| {
                el.child(div().text_xs().text_color(muted).child(say!(cx, "renders-strip-saving")))
            });
        // A plain scroll area: gpui-kit's `overflow_x_scrollbar` locks to
        // the gesture's axis, so a vertical wheel couldn't move the strip.
        // Unlocked, GPUI maps the wheel onto the one scrollable axis.
        v_flex()
            .w_full()
            .flex_shrink_0()
            .bg(bg)
            .border_t_1()
            .border_color(border)
            .child(heading)
            .child(
                div()
                    .relative()
                    .w_full()
                    .min_w_0()
                    .child(
                        h_flex()
                            .id("history-strip")
                            .w_full()
                            .px_2()
                            .py_1p5()
                            .gap(px(TILE_GAP))
                            .items_start()
                            .overflow_x_scroll()
                            .track_scroll(&self.scroll)
                            .children(tiles),
                    )
                    .scrollbar(&self.scroll, ScrollbarAxis::Horizontal),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use chrono::NaiveDate;
    use deptheos_app::SessionOptions;
    use image::RgbImage;

    use super::labels::{age, day_label, frame_range, frames_summary};
    use super::queue::{SaveRequest, strip_thumbnail};
    use super::tiles::thumb_width;
    use super::*;
    use crate::words::DesktopWords;

    fn range(names: &[&str], included: &[bool]) -> String {
        let frames: Vec<PathBuf> =
            names.iter().map(|n| PathBuf::from(format!("/shoot/{n}"))).collect();
        frame_range(&frames, included)
    }

    #[test]
    fn frame_ranges_leave_out_excluded_ends() {
        assert_eq!(
            range(&["IMG_0120.CR3", "IMG_0121.CR3", "IMG_0160.CR3"], &[true; 3]),
            "IMG_0120–160"
        );
        assert_eq!(
            range(&["a1.png", "a2.png", "a3.png", "a4.png"], &[false, true, true, false]),
            "a2–3"
        );
        assert_eq!(range(&["only.tif"], &[true]), "only");
    }

    #[test]
    fn labels_read_like_the_sliders() {
        let w = DesktopWords::english();
        let defaults = SessionOptions::default();
        assert_eq!(frames_summary(&w, &[true; 3], false, &defaults), "3 frames");
        let alignment = deptheos_app::Alignment::On(deptheos_app::AlignSettings {
            mode: deptheos_app::AlignMode::Grid,
            crop: false,
            ..Default::default()
        });
        let grid = SessionOptions { alignment, ..defaults };
        assert_eq!(
            frames_summary(&w, &[true, false, true], true, &grid),
            "2 of 3 frames · RAW · Grid search · uncropped"
        );
        let off = SessionOptions {
            alignment: deptheos_app::Alignment::Off { resumes: alignment.settings() },
            ..defaults
        };
        assert_eq!(frames_summary(&w, &[true; 2], false, &off), "2 frames · not aligned");
        assert_eq!(thumb_width(300, 200), 90.0);
        assert_eq!(thumb_width(100, 1000), THUMB_WIDTH.0);
    }

    #[test]
    fn days_read_today_yesterday_then_dates() {
        let w = DesktopWords::english();
        let today = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let day = |y, m, d| day_label(&w, NaiveDate::from_ymd_opt(y, m, d).unwrap(), today);
        assert_eq!(day_label(&w, today, today), "Today");
        assert_eq!(day(2026, 10, 31), "Yesterday");
        assert_eq!(day(2026, 10, 28), "Oct 28");
        assert_eq!(day(2025, 3, 2), "Mar 2, 2025");
    }

    #[test]
    fn ages_are_coarse_and_plural() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000_000);
        let w = DesktopWords::english();
        let ago = |seconds: u64| age(&w, now - Duration::from_secs(seconds), now);
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(90), "1 minute ago");
        assert_eq!(ago(3 * 3600), "3 hours ago");
        assert_eq!(ago(20 * 86_400), "2 weeks ago");
        assert_eq!(age(&w, now + Duration::from_secs(60), now), "just now");
    }

    #[test]
    fn a_failed_write_says_what_it_was() {
        let render = RenderId::parse(&format!("{:032x}", 1)).unwrap();
        let w = DesktopWords::english();
        let pick = |picked| SaveRequest::Pick { render: render.clone(), picked };
        assert_eq!(pick(true).failed(&w), "The render couldn't be starred.");
        assert_eq!(pick(false).failed(&w), "The render couldn't be unstarred.");
        let relink = SaveRequest::Relink { old: "/a".into(), new: "/b".into() };
        assert_eq!(relink.failed(&w), "The renders couldn't be pointed at the new folder.");
    }

    #[test]
    fn thumbnails_shrink_to_the_strip_in_bgra() {
        let image = RgbImage::from_pixel(600, 400, image::Rgb([10, 20, 30]));
        let small = strip_thumbnail(&image);
        assert_eq!((small.width, small.height), (252, THUMB_DECODE_HEIGHT));
        assert_eq!(&small.pixels[..4], &[30, 20, 10, 255]);
    }
}
