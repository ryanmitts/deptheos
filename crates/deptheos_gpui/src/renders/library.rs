//! The Library screen: every stack, newest day first, a batch run's in a
//! section of its own. A tile is a stack: its starred (else newest) render
//! and its render count. The left bar filters (All, Starred, Failed), jumps
//! to a period and opens Recently Deleted (`deleted`); the details pane
//! lists the selected stack's renders.
//!
//! Selection follows the Finder (click, ⌘-click, ⇧-click, arrows); Space
//! shows the cover large ([`Preview`]); double-click, ↩ or Open opens the
//! stack's working version ([`LibraryEvent::OpenStack`]). A render opened
//! from the details pane opens its stack showing that render
//! ([`LibraryEvent::Open`]); neither changes the working version.

#[path = "library_deleted.rs"]
mod deleted;
#[path = "library_details.rs"]
mod details;
#[path = "library_layout.rs"]
mod layout;

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDate};
use deptheos_app::CpuImage;
use deptheos_app::library::history::{self, Batch, RenderId, StackId};
use deptheos_app::library::stacks;
use gpui::prelude::*;
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, Div, Entity, EventEmitter, FontWeight, ListAlignment,
    ListOffset, ListState, Modifiers, ObjectFit, Pixels, RenderImage, SharedString, Subscription,
    Task, Window, canvas, div, img, list, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use layout::{
    Above, Across, BatchHeader, Grid, Item, ItemKey, Kind, LEFT_BAR_WIDTH, Row, RowKey, Tile,
    decoder_choice, fit_within, head_of, rows_wanted, stepped,
};

use super::labels::day_label;
use super::menus::entity_item;
use super::tiles::{edited_pill, overlay_pill, render_title, thumb_box};
use super::{LibraryRead, Renders};
use crate::AppIcon;
use crate::images::render_image;
use crate::look::{Fill, Pickable as _, hud_text, overlay_surface};
use crate::multi_select::{Click, MultiSelect};
use crate::words::{DesktopWords, Wording as _, say};

/// A tile's thumbnail box, in points.
const TILE_WIDTH: f32 = 140.0;
const TILE_THUMB_HEIGHT: f32 = 100.0;
/// How far a tile's hover and selection tint reach past its picture.
const TILE_INSET: f32 = 4.0;
/// The details pane's width.
pub const DETAILS_WIDTH: f32 = 272.0;

/// What the grid shows: every stack, the starred, or failed batch stacks.
pub use deptheos_app::library::Filter;

/// The stack open in Stacking, as the app view last said; its working
/// version is live, not kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrentStack {
    pub stack: Option<StackId>,
    pub base: Option<RenderId>,
    /// "Edited since #4", when it is.
    pub edited: Option<String>,
    /// Frames and no render yet: the store doesn't list it, so it is
    /// listed from here.
    pub unstacked: Option<UnstackedNow>,
}

/// The open stack's frames, never stacked.
#[derive(Clone, Debug, PartialEq)]
pub struct UnstackedNow {
    pub name: String,
    /// It has a name (given, or its folder's); the unnamed sort last.
    pub named: bool,
    pub frames: usize,
    pub first: Option<PathBuf>,
}

/// An arrow key's move: along the grid, or a row up or down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Previous,
    Next,
    Up,
    Down,
}

pub enum LibraryEvent {
    /// Open this render, or try this failed stack again, in Stacking.
    Open(RenderId),
    /// Open this stack's working version, in Stacking.
    OpenStack(StackId),
    /// Open this render and export it.
    Export(RenderId),
}

pub struct Library {
    renders: Entity<Renders>,
    selection: Selection,
    /// Changed only by [`Library::set_page`].
    page: deleted::Page,
    /// The batch runs folded away, by batch id.
    collapsed: HashSet<String>,
    /// The search field, which the title bar draws.
    pub search: Entity<InputState>,
    query: String,
    /// The grid's rows, which the list draws only those of in view.
    list: ListState,
    /// The list's rows as keys ([`Row::key`]), changed only by
    /// [`Self::set_rows`]; each frame derives the same rows to draw.
    rows: Vec<RowKey>,
    /// The window's width, which sets how many tiles a row holds.
    width: Pixels,
    current: CurrentStack,
    details_shown: bool,
    decoder_choice: bool,
    preview: Preview,
    /// The details pane's whole picture of its render; the strip's
    /// thumbnail stands in until it is read.
    ///
    /// Lifecycle: `None` (no render shown, or hidden) or `Some` [`Picture`]
    /// for the render the pane shows ([`Library::sync_cover`]), changed only
    /// by [`Library::set_cover`]; shared with the preview
    /// ([`Library::picture_read`]).
    cover: Option<Picture>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LibraryEvent> for Library {}

/// What is selected: stacks in the grid, or one render in the details pane.
///
/// Lifecycle: `Nothing`, `Stacks` (a [`MultiSelect`] with the Finder's
/// rules) or a `Render`, changed only through [`Library::set_selection`].
/// `Nothing` → `Stacks` by a click, ⌘A, an arrow, or showing the Library
/// (the open stack). `Stacks` → `Render` by a click in the details pane;
/// `Render` → `Stacks` by a tile click or an arrow. Either → `Nothing` by
/// Esc, a click on empty grid, or ⌘-click on the only one selected. Stacks
/// gone from the history are dropped as read ([`Library::selected_stacks`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Selection {
    #[default]
    Nothing,
    /// One or more stacks, never none; ⇧-click ranges from the anchor in
    /// grid order.
    Stacks(MultiSelect<StackId>),
    /// `render` of `stack`, the one stack selected: ↩ opens the render.
    Render { stack: StackId, render: RenderId },
}

impl Selection {
    /// `stacks` selected: nothing when it holds none.
    fn of(stacks: MultiSelect<StackId>) -> Self {
        match stacks.is_empty() {
            true => Self::Nothing,
            false => Self::Stacks(stacks),
        }
    }

    /// `stack` alone.
    fn one(stack: StackId) -> Self {
        Self::Stacks(MultiSelect::one(stack))
    }

    /// The stacks selected and the anchor: a render's is its stack.
    fn stacks(&self) -> Option<(&[StackId], &StackId)> {
        match self {
            Self::Nothing => None,
            Self::Stacks(stacks) => Some((stacks.ids(), stacks.anchor()?)),
            Self::Render { stack, .. } => Some((std::slice::from_ref(stack), stack)),
        }
    }

    /// Where a tile click starts from: a render's is its stack alone.
    fn picked(&self) -> MultiSelect<StackId> {
        match self {
            Self::Nothing => MultiSelect::none(),
            Self::Stacks(stacks) => stacks.clone(),
            Self::Render { stack, .. } => MultiSelect::one(stack.clone()),
        }
    }
}

/// A render's whole picture: the store's larger thumbnail
/// ([`Renders::read_picture`]). The cover and Space's [`Preview`] share one
/// load: each takes the other's picture rather than read it again
/// ([`Library::picture_read`]).
///
/// `Loading` → `Shown` once read; a read nobody waits for any more is dropped.
enum Picture {
    Loading {
        render: RenderId,
        _read: Task<()>,
    },
    /// `image` is none when it couldn't be read.
    Shown {
        render: RenderId,
        image: Option<Arc<RenderImage>>,
    },
}

impl Picture {
    fn render(&self) -> &RenderId {
        match self {
            Self::Loading { render, .. } | Self::Shown { render, .. } => render,
        }
    }

    /// The picture, once read.
    fn image(&self) -> Option<&Arc<RenderImage>> {
        match self {
            Self::Shown { image, .. } => image.as_ref(),
            Self::Loading { .. } => None,
        }
    }

    /// The picture shown of `render`, if this is it.
    fn image_of(&self, render: &RenderId) -> Option<&Arc<RenderImage>> {
        self.image().filter(|_| self.render() == render)
    }

    /// Still reading `render`'s picture.
    fn waits_for(&self, render: &RenderId) -> bool {
        matches!(self, Self::Loading { render: wanted, .. } if wanted == render)
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading { render, .. } => write!(f, "Loading({})", render.as_str()),
            Self::Shown { render, image } => {
                write!(f, "Shown({}, picture: {})", render.as_str(), image.is_some())
            }
        }
    }
}

/// Which of the two holds a [`Picture`].
#[derive(Clone, Copy, Debug)]
enum Holder {
    Cover,
    Preview,
}

/// Space's look at one stack: its cover fitted in the window, never past
/// its own pixels, with its name under it.
///
/// Lifecycle: `Closed` or `Open` on a stack, changed only by
/// [`Library::set_preview`]. `Closed` → `Open` by Space with a stack
/// selected (a stack without render, or an unreadable cover, shows the
/// placeholder). An arrow moves it to the next stack; Space, Esc, a click,
/// opening a stack, leaving the Library or the stack leaving the grid close it.
#[derive(Default)]
enum Preview {
    #[default]
    Closed,
    Open {
        stack: StackId,
        picture: Option<Picture>,
    },
}

impl Preview {
    fn stack(&self) -> Option<&StackId> {
        match self {
            Self::Closed => None,
            Self::Open { stack, .. } => Some(stack),
        }
    }

    fn picture(&self) -> Option<&Picture> {
        match self {
            Self::Closed => None,
            Self::Open { picture, .. } => picture.as_ref(),
        }
    }
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "Closed"),
            Self::Open { stack, picture } => write!(f, "Open({}, {picture:?})", stack.as_str()),
        }
    }
}

impl Library {
    pub fn new(renders: Entity<Renders>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = say!(cx, "library-search");
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscriptions = vec![
            cx.observe_window_bounds(window, |this, window, cx| {
                let width = window.viewport_size().width;
                if width != this.width {
                    this.width = width;
                    this.grid_changed(cx);
                }
            }),
            cx.observe(&renders, |this, _, cx| {
                this.grid_changed(cx);
                this.deleted_maybe_changed(cx);
                cx.notify();
            }),
            cx.observe_global::<crate::prefs::Prefs>(|this, cx| {
                this.decoder_choice = decoder_choice();
                cx.notify();
            }),
            cx.subscribe_in(&search, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    this.query = input.read(cx).value().to_string();
                    this.grid_changed(cx);
                    this.list.scroll_to(ListOffset::default());
                    cx.notify();
                }
            }),
        ];
        Self {
            renders,
            selection: Selection::Nothing,
            page: deleted::Page::Grid(Filter::All),
            collapsed: HashSet::new(),
            search,
            query: String::new(),
            // A row or two past the window is measured ahead.
            list: ListState::new(0, ListAlignment::Top, px(300.0)),
            rows: Vec::new(),
            width: window.viewport_size().width,
            current: CurrentStack::default(),
            details_shown: true,
            decoder_choice: decoder_choice(),
            preview: Preview::Closed,
            cover: None,
            _subscriptions: subscriptions,
        }
    }

    /// What Stacking has open and whether the details pane is shown, set by
    /// the app view on change, never as it draws.
    pub fn set_current(
        &mut self,
        current: CurrentStack,
        details_shown: bool,
        cx: &mut Context<Self>,
    ) {
        if self.current != current || self.details_shown != details_shown {
            self.current = current;
            self.details_shown = details_shown;
            self.grid_changed(cx);
        }
    }

    /// What the grid shows may have changed: follow it here, never as the
    /// Library draws. The preview closes on a stack no longer shown, the
    /// details pane reads its render's picture, and the list takes the rows.
    fn grid_changed(&mut self, cx: &mut Context<Self>) {
        let grid = self.grid(cx);
        if self.preview.stack().is_some_and(|s| grid.tile_of(s).is_none()) {
            self.close_preview(cx);
        }
        self.sync_cover(&grid, cx);
        let rows = grid.rows(&self.collapsed, self.across());
        self.set_rows(rows.iter().map(Row::key).collect());
    }

    /// Tiles per row at the window's width beside the bar and details pane.
    fn across(&self) -> Across {
        Across::of(self.width, self.details_shown)
    }

    /// The only way [`Self::rows`] changes: replace the rows that changed
    /// and measure them as they come into view, keeping the scroll on the
    /// row where the top one was.
    fn set_rows(&mut self, rows: Vec<RowKey>) {
        if rows == self.rows {
            return;
        }
        let old = std::mem::replace(&mut self.rows, rows);
        let new = &self.rows;
        let head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let tail = old[head..]
            .iter()
            .rev()
            .zip(new[head..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let replaced = head..old.len() - tail;
        let top = self.list.logical_scroll_top();
        let stays = old
            .get(top.item_ix)
            .filter(|_| replaced.contains(&top.item_ix))
            .and_then(|was| new.iter().position(|row| row.replaces(was)));
        tracing::debug!(
            "library rows: {} -> {}, {} from row {head}",
            old.len(),
            new.len(),
            replaced.len()
        );
        self.list.splice(replaced, new.len() - head - tail);
        if let Some(item_ix) = stays {
            self.list.scroll_to(ListOffset { item_ix, offset_in_item: top.offset_in_item });
        }
    }

    // ---------------------------------------------------------- selection

    fn set_selection(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if self.selection != selection {
            tracing::debug!("library selection: {:?} -> {selection:?}", self.selection);
            self.selection = selection;
            let grid = self.grid(cx);
            self.sync_cover(&grid, cx);
            cx.notify();
        }
    }

    /// Show only `show`'s stacks (all, starred or failed), from the top.
    pub fn show_only(&mut self, show: Filter, cx: &mut Context<Self>) {
        self.set_page(deleted::Page::Grid(show), cx);
        self.list.scroll_to(ListOffset::default());
        cx.notify();
    }

    /// Select `stack`, the one open in Stacking, as the Library is shown,
    /// so ↩ goes back to it.
    pub fn select_open(&mut self, stack: StackId, cx: &mut Context<Self>) {
        self.set_selection(Selection::one(stack), cx);
    }

    /// The history is read and empty, Recently Deleted too: the first run.
    pub fn is_blank(&self, cx: &gpui::App) -> bool {
        let renders = self.renders.read(cx);
        matches!(renders.listing(), LibraryRead::Listed)
            && self.stack_count(cx) == 0
            && renders.trash().is_none_or(|(records, _)| records == 0)
    }

    /// The empty grid's text: still reading (never "No stacks yet" before),
    /// couldn't read, none, or nothing matches.
    pub(crate) fn empty_text(&self, cx: &gpui::App) -> String {
        match self.renders.read(cx).listing() {
            LibraryRead::Listing { .. } => say!(cx, "library-reading"),
            LibraryRead::Failed if self.stack_count(cx) == 0 => say!(cx, "library-unread"),
            LibraryRead::Listed if self.stack_count(cx) == 0 => say!(cx, "library-empty"),
            LibraryRead::Listed | LibraryRead::Failed => match self.page.filter() {
                _ if !self.query.trim().is_empty() => {
                    say!(cx, "library-no-match-search", query = self.query.trim().to_string())
                }
                Filter::All => say!(cx, "library-empty"),
                Filter::Starred => say!(cx, "library-no-match-starred"),
                Filter::Failed => say!(cx, "library-no-match-failed"),
            },
        }
    }

    /// The stacks selected that the grid still shows, in its order.
    fn selected_stacks(&self, cx: &gpui::App) -> Vec<StackId> {
        self.selected_in(&self.grid(cx))
    }

    /// [`Self::selected_stacks`], in this frame's `grid`.
    fn selected_in(&self, grid: &Grid) -> Vec<StackId> {
        let Some((stacks, _)) = self.selection.stacks() else { return Vec::new() };
        grid.order(&self.collapsed).into_iter().filter(|s| stacks.contains(s)).collect()
    }

    /// The one stack selected, when it is one.
    #[cfg(test)]
    pub fn selected_stack(&self, cx: &gpui::App) -> Option<StackId> {
        match &self.selected_stacks(cx)[..] {
            [stack] => Some(stack.clone()),
            _ => None,
        }
    }

    /// Select `render` of `stack` in the details pane, as a click does.
    #[cfg(test)]
    pub fn select_render(&mut self, stack: &StackId, render: &RenderId, cx: &mut Context<Self>) {
        self.click_render(stack, render, 1, cx);
    }

    /// The selected stacks' names, in the grid's order.
    #[cfg(test)]
    pub fn selected_titles(&self, cx: &gpui::App) -> Vec<String> {
        let grid = self.grid(cx);
        let selected = self.selected_in(&grid);
        selected.iter().filter_map(|s| grid.tile_of(s).map(|t| t.title.clone())).collect()
    }

    /// What the preview shows: "Closed", "Loading bees", "Shown bees" or
    /// "Shown ants (placeholder)".
    #[cfg(test)]
    pub fn preview_text(&self, cx: &gpui::App) -> String {
        let title = |stack: &StackId| self.tile_of(stack, cx).map(|t| t.title).unwrap_or_default();
        match &self.preview {
            Preview::Closed => "Closed".into(),
            Preview::Open { stack, picture: Some(Picture::Loading { .. }) } => {
                format!("Loading {}", title(stack))
            }
            Preview::Open { stack, picture: Some(Picture::Shown { image: Some(_), .. }) } => {
                format!("Shown {}", title(stack))
            }
            Preview::Open { stack, .. } => format!("Shown {} (placeholder)", title(stack)),
        }
    }

    /// The size of the picture the preview shows, in pixels.
    #[cfg(test)]
    pub fn preview_picture_size(&self) -> Option<(i32, i32)> {
        let size = self.preview.picture()?.image()?.size(0);
        Some((size.width.0, size.height.0))
    }

    /// The preview draws the very picture the details pane does.
    #[cfg(test)]
    pub fn preview_shares_cover(&self) -> bool {
        let image = |p: Option<&Picture>| p.and_then(Picture::image).cloned();
        match (image(self.preview.picture()), image(self.cover.as_ref())) {
            (Some(a), Some(b)) => Arc::ptr_eq(&a, &b),
            _ => false,
        }
    }

    /// The details pane's render and picture size in pixels; `None` while
    /// the strip's thumbnail stands in.
    #[cfg(test)]
    pub fn cover_picture(&self) -> Option<(RenderId, (i32, i32))> {
        let picture = self.cover.as_ref()?;
        let size = picture.image()?.size(0);
        Some((picture.render().clone(), (size.width.0, size.height.0)))
    }

    /// How far the grid is scrolled down.
    #[cfg(test)]
    pub fn scrolled(&self) -> Pixels {
        -self.list.scroll_px_offset_for_scrollbar().y
    }

    /// Scroll the grid to its end, as far as it goes.
    #[cfg(test)]
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Each tile's cover, in the grid's order.
    #[cfg(test)]
    pub fn shown_covers(&self, cx: &gpui::App) -> Vec<RenderId> {
        let sections = self.grid(cx).sections;
        let tiles = sections.iter().flat_map(|s| s.runs.iter().flat_map(|r| r.tiles().to_vec()));
        tiles
            .filter_map(|t| match t.kind {
                Kind::Renders { cover, .. } => Some(cover),
                _ => None,
            })
            .collect()
    }

    /// How many stacks are selected.
    #[cfg(test)]
    pub fn selected_len(&self, cx: &gpui::App) -> usize {
        self.selected_stacks(cx).len()
    }

    fn is_selected(&self, stack: &StackId) -> bool {
        self.selection.stacks().is_some_and(|(stacks, _)| stacks.contains(stack))
    }

    /// A click on `stack`'s tile, by the Finder's rules ([`MultiSelect`]).
    fn click(&mut self, stack: &StackId, modifiers: Modifiers, cx: &mut Context<Self>) {
        let mut picked = self.selection.picked();
        picked.click(stack, Click::of(modifiers), &self.order(cx));
        self.set_selection(Selection::of(picked), cx);
    }

    /// ⌘A: every stack the filter and search let through.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            return self.select_all_deleted(true, cx);
        }
        let stacks = self.order(cx);
        self.set_selection(Selection::of(MultiSelect::of(stacks)), cx);
    }

    /// Esc, or a click on empty grid: nothing selected.
    pub fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            return self.select_all_deleted(false, cx);
        }
        self.set_selection(Selection::Nothing, cx);
    }

    /// An arrow key: select the stack `step` away, or with `extend` (⇧)
    /// range from the anchor, and scroll it into view. ↑ and ↓ go by rows,
    /// or along the renders when one is selected in the details pane.
    pub fn step(&mut self, step: Step, extend: bool, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            return;
        }
        if self.details_shown
            && !extend
            && matches!(step, Step::Up | Step::Down)
            && let Selection::Render { stack, render } = &self.selection
        {
            let renders = self.renders.read(cx);
            let mut of_stack: Vec<&history::Render> =
                renders.renders().iter().filter(|o| o.stack == *stack).collect();
            of_stack.sort_by_key(|o| std::cmp::Reverse(o.number));
            if let Some(at) = of_stack.iter().position(|o| o.id == *render) {
                let to = match step {
                    Step::Up => at.saturating_sub(1),
                    Step::Down => (at + 1).min(of_stack.len() - 1),
                    _ => at,
                };
                let next =
                    Selection::Render { stack: stack.clone(), render: of_stack[to].id.clone() };
                self.set_selection(next, cx);
                return;
            }
        }
        let order = self.order(cx);
        let rows: Vec<Vec<StackId>> =
            self.rows.iter().filter_map(RowKey::stacks).map(<[StackId]>::to_vec).collect();
        let (anchor, head) = match self.selection.stacks() {
            Some((stacks, anchor)) => (Some(anchor), head_of(&order, stacks, anchor)),
            None => (None, None),
        };
        let Some(to) = stepped(&rows, head.as_ref(), step) else { return };
        // ⇧ extends from the anchor as ⇧-click does.
        let mut picked = self.selection.picked();
        let click = if extend && anchor.is_some() { Click::Range } else { Click::Plain };
        picked.click(&to, click, &order);
        self.set_selection(Selection::of(picked), cx);
        self.scroll_to(&to, cx);
        if self.is_previewing() {
            self.preview_stack(&to, cx);
        }
    }

    // -------------------------------------------------------------- cover

    /// The only way [`Self::cover`] changes.
    fn set_cover(&mut self, cover: Option<Picture>, cx: &mut Context<Self>) {
        tracing::debug!("library cover: {:?} -> {cover:?}", self.cover);
        let left = std::mem::replace(&mut self.cover, cover);
        self.release(left.as_ref(), cx);
        cx.notify();
    }

    /// A picture no longer held leaves the atlas, unless the other holder
    /// shows it.
    fn release(&self, left: Option<&Picture>, cx: &mut Context<Self>) {
        let Some(image) = left.and_then(Picture::image) else { return };
        let held = [self.cover.as_ref(), self.preview.picture()]
            .into_iter()
            .flatten()
            .filter_map(Picture::image)
            .any(|i| Arc::ptr_eq(i, image));
        if !held {
            let image = image.clone();
            cx.defer(move |cx| cx.drop_image(image, None));
        }
    }

    /// `render`'s picture for `holder`: the other holder's, or read.
    fn picture_of(&self, render: RenderId, holder: Holder, cx: &mut Context<Self>) -> Picture {
        let other = match holder {
            Holder::Cover => self.preview.picture(),
            Holder::Preview => self.cover.as_ref(),
        };
        if let Some(image) = other.and_then(|p| p.image_of(&render)) {
            tracing::debug!("library {holder:?}: taking the picture shown of {}", render.as_str());
            return Picture::Shown { render, image: Some(image.clone()) };
        }
        let read = self.renders.read(cx).read_picture(&render);
        let asked = render.clone();
        let read = cx.spawn(async move |this, cx| {
            let image = read.await;
            this.update(cx, |this, cx| this.picture_read(&asked, image, cx)).ok();
        });
        Picture::Loading { render, _read: read }
    }

    /// `render`'s picture was read: the cover and preview take it if waiting.
    fn picture_read(&mut self, render: &RenderId, image: Option<CpuImage>, cx: &mut Context<Self>) {
        let cover = self.cover.as_ref().is_some_and(|p| p.waits_for(render));
        let preview = self.preview.picture().is_some_and(|p| p.waits_for(render));
        if !cover && !preview {
            return;
        }
        let image = image.map(render_image);
        let shown = || Picture::Shown { render: render.clone(), image: image.clone() };
        if cover {
            self.set_cover(Some(shown()), cx);
        }
        if let (true, Some(stack)) = (preview, self.preview.stack().cloned()) {
            self.set_preview(Preview::Open { stack, picture: Some(shown()) }, cx);
        }
        cx.notify();
    }

    /// The details pane's picture follows `grid`; none while hidden.
    fn sync_cover(&mut self, grid: &Grid, cx: &mut Context<Self>) {
        let wanted = self.details_render(grid, cx).filter(|_| self.details_shown);
        self.want_cover(wanted, cx);
    }

    /// The details pane shows `render`: read its picture unless already
    /// read or reading.
    fn want_cover(&mut self, render: Option<RenderId>, cx: &mut Context<Self>) {
        if self.cover.as_ref().map(Picture::render) == render.as_ref() {
            return;
        }
        let next = render.map(|render| self.picture_of(render, Holder::Cover, cx));
        self.set_cover(next, cx);
    }

    /// The details pane's picture of `render`, or the strip's thumbnail
    /// until read.
    fn cover_image(&self, render: &RenderId, cx: &gpui::App) -> Option<Arc<RenderImage>> {
        match self.cover.as_ref().and_then(|p| p.image_of(render)) {
            Some(image) => Some(image.clone()),
            None => self.renders.read(cx).thumbnail(render),
        }
    }

    /// The details pane's render: the one selected in it, if of the one
    /// stack selected, else that stack's cover.
    fn details_render(&self, grid: &Grid, cx: &gpui::App) -> Option<RenderId> {
        let [stack] = &self.selected_in(grid)[..] else { return None };
        let Kind::Renders { cover, .. } = &grid.tile_of(stack)?.kind else { return None };
        let renders = self.renders.read(cx);
        let selected =
            self.selected_render().filter(|id| renders.find(id).is_some_and(|r| r.stack == *stack));
        Some(selected.unwrap_or(cover).clone())
    }

    // ------------------------------------------------------------ preview

    fn set_preview(&mut self, preview: Preview, cx: &mut Context<Self>) {
        tracing::debug!("library preview: {:?} -> {preview:?}", self.preview);
        // A picture shown leaves the sprite atlas with it.
        let left = std::mem::replace(&mut self.preview, preview);
        if let Preview::Open { picture, .. } = &left {
            self.release(picture.as_ref(), cx);
        }
        cx.notify();
    }

    /// Whether Space's preview is open.
    pub fn is_previewing(&self) -> bool {
        !matches!(self.preview, Preview::Closed)
    }

    /// Space: show the selected stack's cover large (the last moved to), or
    /// close it.
    pub fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        if self.close_preview(cx) || self.is_showing_deleted() {
            return;
        }
        let order = self.order(cx);
        let Some((stacks, anchor)) = self.selection.stacks() else { return };
        if let Some(stack) = head_of(&order, stacks, anchor) {
            self.preview_stack(&stack, cx);
        }
    }

    /// Close the preview: whether it was open.
    pub fn close_preview(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.is_previewing();
        if open {
            self.set_preview(Preview::Closed, cx);
        }
        open
    }

    /// Preview `stack`: its cover once its picture is in.
    fn preview_stack(&mut self, stack: &StackId, cx: &mut Context<Self>) {
        let Some(tile) = self.tile_of(stack, cx) else { return };
        let picture = match tile.kind {
            Kind::Renders { cover, .. } => Some(self.picture_of(cover, Holder::Preview, cx)),
            Kind::Failed { .. } | Kind::Unstacked { .. } => None,
        };
        self.set_preview(Preview::Open { stack: stack.clone(), picture }, cx);
    }

    /// The preview: the cover fitted ([`fit_within`]) or the placeholder,
    /// with the name under it; a click closes it.
    fn render_preview(&self, grid: &Grid, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tile = grid.tile_of(self.preview.stack()?)?;
        let theme = cx.theme();
        let picture = match (self.preview.picture().and_then(Picture::image), &tile.kind) {
            (Some(image), _) => {
                let (image, radius) = (image.clone(), theme.radius);
                canvas(
                    |_, _, _| {},
                    move |room, _, window, _| {
                        let size = image.size(0);
                        let own = gpui::size(px(size.width.0 as f32), px(size.height.0 as f32));
                        let bounds = fit_within(own, room);
                        let corners = gpui::Corners::all(radius);
                        if let Err(e) =
                            window.paint_image(bounds, bounds, corners, image.clone(), 0, false)
                        {
                            tracing::warn!("couldn't draw the preview: {e}");
                        }
                    },
                )
                .size_full()
                .into_any_element()
            }
            // On the preview's dark backdrop, in its light text.
            (_, Kind::Failed { .. }) => tile_placeholder(
                IconName::TriangleAlert,
                say!(cx, "library-tile-failed"),
                hud_text(),
            ),
            (_, Kind::Unstacked { .. }) => {
                tile_placeholder(IconName::Layers, say!(cx, "library-tile-unstacked"), hud_text())
            }
            // Its cover is being read.
            (_, Kind::Renders { .. }) => div().into_any_element(),
        };
        let title = tile.preview_title.clone();
        Some(
            overlay_surface(Fill::Hud, cx)
                .flex()
                .flex_col()
                .id("library-preview")
                .debug_selector(|| "library-preview".into())
                .absolute()
                .inset_2()
                .p_8()
                .gap_3()
                .items_center()
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_preview(cx);
                }))
                .child(div().flex_1().min_h_0().w_full().child(picture))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(title),
                )
                .into_any_element(),
        )
    }

    /// Scroll the grid the least that shows `stack`'s row whole.
    fn scroll_to(&mut self, stack: &StackId, cx: &mut Context<Self>) {
        let holds = |row: &RowKey| row.stacks().is_some_and(|s| s.contains(stack));
        if let Some(ix) = self.rows.iter().position(holds) {
            self.list.scroll_to_reveal_item(ix);
            cx.notify();
        }
    }

    /// Jump To: scroll the grid to the top of its `day`th day.
    fn jump_to(&mut self, day: usize, cx: &mut Context<Self>) {
        let mut days =
            self.rows.iter().enumerate().filter(|(_, r)| matches!(r.item, ItemKey::Day(_)));
        if let Some((item_ix, _)) = days.nth(day) {
            self.list.scroll_to(ListOffset { item_ix, offset_in_item: px(0.0) });
            cx.notify();
        }
    }

    /// Whether Open has something to open: one stack.
    pub fn can_open(&self, cx: &gpui::App) -> bool {
        !self.is_showing_deleted() && self.selected_stacks(cx).len() == 1
    }

    /// Open the selection (↩, Open), when it is one stack.
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            return;
        }
        let [stack] = &self.selected_stacks(cx)[..] else { return };
        match &self.selection {
            // A render of it, selected in the details pane, while it lasts.
            Selection::Render { render, .. } if self.renders.read(cx).find(render).is_some() => {
                cx.emit(LibraryEvent::Open(render.clone()))
            }
            _ => self.open_stack(stack, cx),
        }
    }

    /// A click on a render of `stack` in the details pane selects it; a
    /// double-click opens it as stacked.
    fn click_render(
        &mut self,
        stack: &StackId,
        render: &RenderId,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let selection = Selection::Render { stack: stack.clone(), render: render.clone() };
        self.set_selection(selection, cx);
        if clicks >= 2 {
            self.close_preview(cx);
            cx.emit(LibraryEvent::Open(render.clone()));
        }
    }

    /// The render selected in the details pane, if one is.
    fn selected_render(&self) -> Option<&RenderId> {
        match &self.selection {
            Selection::Render { render, .. } => Some(render),
            _ => None,
        }
    }

    /// Open `stack` as its tile does: a failed one to retry, else its
    /// working version.
    fn open_stack(&mut self, stack: &StackId, cx: &mut Context<Self>) {
        self.close_preview(cx);
        match self.tile_of(stack, cx).map(|t| t.kind) {
            Some(Kind::Failed { failure }) => cx.emit(LibraryEvent::Open(failure)),
            Some(_) => cx.emit(LibraryEvent::OpenStack(stack.clone())),
            None => {}
        }
    }

    /// Delete the selection (⌘⌫) into Recently Deleted without asking
    /// (ADR-140): the details pane's render, else the selected stacks.
    pub fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_showing_deleted() {
            return;
        }
        if let Some(render) = self.selected_render().cloned() {
            self.renders.update(cx, |r, cx| {
                if r.can_delete() {
                    r.delete(vec![render], window, cx)
                }
            });
            return;
        }
        let stacks = self.selected_stacks(cx);
        self.delete_stacks(stacks, window, cx);
    }

    /// The delete items work: not during a delete, save or stack
    /// ([`Renders::can_delete`]).
    fn can_delete(&self, cx: &gpui::App) -> bool {
        self.renders.read(cx).can_delete()
    }

    /// Delete `stacks` whole, when deleting works now.
    fn delete_stacks(&mut self, stacks: Vec<StackId>, window: &mut Window, cx: &mut Context<Self>) {
        self.renders.update(cx, |r, cx| {
            if r.can_delete() {
                r.delete_stacks(stacks, window, cx)
            }
        });
    }

    /// Rename Stack…: ask for the stack's new name.
    pub(crate) fn rename_stack(
        &mut self,
        stack: StackId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.renders.read(cx).stack_name(&stack, cx.wording());
        let placeholder = say!(cx, "library-rename-placeholder");
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        input.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.select_all(window, cx);
        });
        let renders = self.renders.downgrade();
        let field = input.clone();
        let (title, text, ok) = (
            say!(cx, "library-rename-title"),
            say!(cx, "library-rename-text"),
            say!(cx, "library-rename-ok"),
        );
        let cancel = say!(cx, "button-cancel");
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (renders, field, stack) = (renders.clone(), field.clone(), stack.clone());
            alert
                .title(title.clone())
                .description(text.clone())
                .child(gpui_kit::component::input::Input::new(&field))
                .confirm()
                .ok_text(ok.clone())
                .cancel_text(cancel.clone())
                .on_ok(move |_, _, cx| {
                    let name = field.read(cx).value().to_string();
                    renders.update(cx, |r, cx| r.rename_stack(&stack, &name, cx)).ok();
                    true
                })
        });
        let handle = gpui::Focusable::focus_handle(input.read(cx), cx);
        window.focus(&handle, cx);
    }

    /// After this frame, read thumbnails of the rows drawn (`drawn`), as
    /// many rows again either side ([`rows_wanted`]), then the details
    /// pane's (`details`). Only these are kept past the history's limit.
    ///
    /// The policy hears of it after the frame ([`Renders::request_thumbnails`])
    /// and redraws only when a read lands, so a frame never asks for another.
    fn want_thumbnails(
        &self,
        rows: Rc<Vec<Row>>,
        drawn: Rc<RefCell<Vec<usize>>>,
        details: Vec<RenderId>,
        cx: &mut Context<Self>,
    ) {
        let renders = self.renders.clone();
        cx.defer(move |cx| {
            let wanted = rows_wanted(&drawn.borrow(), rows.len()).into_iter();
            let covers = wanted.flat_map(|ix| rows[ix].covers()).cloned();
            let wanted: Vec<RenderId> = covers.chain(details).collect();
            renders.update(cx, |r, cx| r.request_thumbnails(wanted, cx));
        });
    }

    // ------------------------------------------------------------ drawing

    /// Show and Jump to.
    fn render_left_bar(&self, grid: &Grid, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let heading = |text: String| {
            div()
                .px_2()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted)
                .child(text.to_uppercase())
        };
        // A row dimmed because it is empty takes no hover or hand.
        let row = |id: SharedString, label: String, count: usize, selected: bool, enabled: bool| {
            h_flex()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .text_sm()
                .pickable(selected, enabled, cx)
                .when(!enabled, |el| el.opacity(0.5))
                .child(div().flex_1().child(label))
                .child(div().text_xs().text_color(muted).child(count.to_string()))
        };
        let showing_deleted = self.is_showing_deleted();
        let mut shows = Vec::new();
        // Each with its row's element id.
        for (show, id, label) in [
            (Filter::All, "show-All stacks", say!(cx, "library-show-all")),
            (Filter::Starred, "show-Starred", say!(cx, "library-show-starred")),
            (Filter::Failed, "show-Failed", say!(cx, "library-show-failed")),
        ] {
            shows.push(
                row(
                    id.into(),
                    label,
                    grid.counts.of(show),
                    matches!(self.page, deleted::Page::Grid(f) if f == show),
                    true,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.show_only(show, cx)))
                .into_any_element(),
            );
        }
        let mut jumps = Vec::new();
        for span in grid.spans {
            let first = span.first;
            jumps.push(
                row(
                    format!("jump-{:?}", span.span).into(),
                    span.span.label(cx.wording().shared()),
                    span.stacks,
                    false,
                    first.is_some() && !showing_deleted,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(day) = first {
                        this.jump_to(day, cx);
                    }
                }))
                .into_any_element(),
            );
        }
        // Every failed stack at once: they arrive in clumps from a batch.
        let can_delete = self.can_delete(cx);
        // The failed stacks shown, unless a search leaves some out.
        let delete_failed = (matches!(self.page, deleted::Page::Grid(Filter::Failed))
            && !grid.failed.is_empty())
        .then(|| {
            let stacks = grid.failed.clone();
            let label = match self.query.trim().is_empty() {
                true => say!(cx, "library-delete-all-failed"),
                false => say!(cx, "library-batch-delete-failed", n = stacks.len()),
            };
            div().px_2().pt_3().child(
                Button::new("delete-failed")
                    .label(label)
                    .small()
                    .outline()
                    .w_full()
                    .disabled(!can_delete)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.delete_stacks(stacks.clone(), window, cx)
                    })),
            )
        });
        v_flex()
            .h_full()
            .w(px(LEFT_BAR_WIDTH))
            .flex_shrink_0()
            .p_2()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(border)
            .child(heading(say!(cx, "library-show")))
            .children(shows)
            .children(delete_failed)
            .child(heading(say!(cx, "library-jump-to")))
            .children(jumps)
            // Last, below the filters and the periods.
            .child(self.render_deleted_row(cx))
            .into_any_element()
    }

    /// A stack: its cover, its name, and how many renders it has.
    fn render_tile(&self, tile: Tile, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self.is_selected(&tile.stack);
        let (ring, muted, danger) = (theme.ring, theme.muted_foreground, theme.danger);
        let edited = crate::params::edited_color(cx);
        let thumb = div()
            .relative()
            .w(px(TILE_WIDTH))
            .h(px(TILE_THUMB_HEIGHT))
            .rounded_md()
            .overflow_hidden()
            .border_2()
            .border_color(if selected { ring } else { gpui::transparent_black() })
            .bg(theme.muted);
        let thumb = match &tile.kind {
            Kind::Failed { .. } => thumb.child(tile_placeholder(
                IconName::TriangleAlert,
                say!(cx, "library-tile-failed"),
                danger,
            )),
            Kind::Unstacked { .. } => thumb.child(tile_placeholder(
                IconName::Layers,
                say!(cx, "library-tile-unstacked"),
                muted,
            )),
            Kind::Renders { .. } => thumb.when_some(tile.thumbnail.clone(), |el, image| {
                el.child(img(image).size_full().object_fit(ObjectFit::Cover))
            }),
        };
        let thumb = thumb
            .when(tile.edited, |el| el.child(edited_pill(cx, edited).top_1().left_1()))
            // How many renders the stack holds, when it is more than one.
            .when_some(tile.badge.clone(), |el, count| {
                el.child(
                    overlay_pill(cx)
                        .bottom_1()
                        .right_1()
                        .child(Icon::new(AppIcon::Render).xsmall())
                        .child(count),
                )
            });
        // The star is on the cover's number, in the detail line.
        let title = tile.title.clone();
        let this = cx.weak_entity();
        let stack = tile.stack.clone();
        let menu_tile = tile.clone();
        let selector = match tile.kind {
            Kind::Unstacked { .. } => format!("library-unstacked-{}", tile.title),
            _ => format!("library-tile-{}", tile.title),
        };
        // Padded past its column (the margin takes it back), so the tint
        // frames it without moving the grid.
        v_flex()
            .id(SharedString::from(format!("library-{}", tile.stack.as_str())))
            .debug_selector(move || selector.clone())
            .w(px(TILE_WIDTH + 2.0 * TILE_INSET))
            .m(px(-TILE_INSET))
            .p(px(TILE_INSET))
            .rounded(cx.theme().radius_lg)
            .gap_0p5()
            .pickable(selected, true, cx)
            .child(thumb)
            .child(div().mt_1().text_sm().font_weight(FontWeight::MEDIUM).truncate().child(title))
            .child(div().text_xs().text_color(muted).truncate().child(tile.detail))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                // Not a click on empty grid, which selects nothing.
                cx.stop_propagation();
                let modifiers = event.modifiers();
                this.click(&stack, modifiers, cx);
                if event.click_count() >= 2 && !modifiers.secondary() && !modifiers.shift {
                    this.open_stack(&stack, cx);
                }
            }))
            .context_menu(move |menu, _, cx| {
                // A right-click outside the selection selects that tile.
                let several = this
                    .update(cx, |this, cx| {
                        if !this.is_selected(&menu_tile.stack) {
                            this.click(&menu_tile.stack, Modifiers::none(), cx);
                        }
                        this.selected_stacks(cx)
                    })
                    .unwrap_or_default();
                let renders = this.upgrade().map(|l| l.read(cx).renders.clone());
                let Some(renders) = renders else { return menu };
                if several.len() > 1 {
                    let can_delete = renders.read(cx).can_delete();
                    return several_menu(menu, this.clone(), several, can_delete, cx.wording());
                }
                tile_menu(menu, this.clone(), renders, &menu_tile, cx)
            })
            .into_any_element()
    }

    /// A batch's header: label, count and date, a menu, and the first
    /// covers while folded.
    fn render_batch_header(&self, header: &BatchHeader, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let BatchHeader { batch, started, failed, stacks, failures, peeks, collapsed } = header;
        let time = crate::dates::time(DateTime::<Local>::from(*started));
        let about = match *failed {
            0 => say!(cx, "library-batch-about", n = stacks.len(), time = time),
            failed => say!(
                cx,
                "library-batch-about-failed",
                n = stacks.len(),
                time = time,
                failed = failed
            ),
        };
        let (all, failures) = (stacks.clone(), failures.clone());
        let this = cx.weak_entity();
        let batch_id = batch.id.clone();
        h_flex()
            .id(SharedString::from(format!("batch-header-{}", batch.id)))
            .gap_2()
            .items_center()
            // Its buttons fold and delete; a click on it keeps the selection.
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                Button::new(SharedString::from(format!("fold-{}", batch.id)))
                    .icon(if *collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.collapsed.remove(&batch_id) {
                            this.collapsed.insert(batch_id.clone());
                        }
                        this.grid_changed(cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(batch.label.text(crate::words::of(cx))),
            )
            .child(div().text_xs().text_color(muted).whitespace_nowrap().child(about))
            .when(*collapsed, |el| {
                el.child(h_flex().ml_2().gap_1().children(peeks.iter().cloned().map(|image| {
                    img(image).w(px(36.0)).h(px(24.0)).rounded_sm().object_fit(ObjectFit::Cover)
                })))
            })
            .child(div().flex_1())
            .child(
                Button::new(SharedString::from(format!("batch-menu-{}", batch.id)))
                    .icon(IconName::Ellipsis)
                    .ghost()
                    .xsmall()
                    .dropdown_menu(move |menu, _, cx| {
                        let can_delete = this.upgrade().is_some_and(|l| l.read(cx).can_delete(cx));
                        let delete = |label: String, stacks: Vec<StackId>| {
                            let disabled = stacks.is_empty() || !can_delete;
                            entity_item(label, &this, move |l, window, cx| {
                                l.delete_stacks(stacks.clone(), window, cx)
                            })
                            .disabled(disabled)
                        };
                        let failed = say!(cx, "library-batch-delete-failed", n = failures.len());
                        let batch = say!(cx, "library-batch-delete", n = all.len());
                        menu.item(delete(failed, failures.clone())).item(delete(batch, all.clone()))
                    }),
            )
            .into_any_element()
    }

    /// A heading with its stack count.
    fn render_heading(&self, title: String, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .gap_2()
            .items_baseline()
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().text_xs().text_color(muted).child(say!(cx, "stacks-count", n = count)))
            .into_any_element()
    }

    /// A list row, framed by its place in a batch's box.
    fn render_row(&self, row: &Row, today: NaiveDate, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let content = match &row.item {
            Item::Unstacked { count } => {
                self.render_heading(say!(cx, "library-unstacked-heading"), *count, cx)
            }
            Item::Day { day, count } => {
                self.render_heading(day_label(cx.wording(), *day, today), *count, cx)
            }
            Item::Batch(header) => self.render_batch_header(header, cx),
            Item::Tiles(tiles) => {
                let tiles = tiles.iter().map(|t| self.render_tile(t.clone(), cx));
                h_flex().gap_4().children(tiles.collect::<Vec<_>>()).into_any_element()
            }
        };
        let spaced = |el: Div, above: Above| match above {
            Above::None => el,
            Above::Run => el.pt_3(),
            Above::Tiles => el.pt_4(),
        };
        let place = row.place;
        let header = matches!(row.item, Item::Batch(_));
        let (content, above) = match place.boxed {
            None => (content, place.above),
            Some(closes) => {
                let edge = div().px_3().border_l_1().border_r_1().border_color(border);
                // The header's space is outside the box, tiles' inside.
                let (edge, above) = match header {
                    true => (edge.pt_3().border_t_1().rounded_t_lg(), place.above),
                    false => (spaced(edge, place.above), Above::None),
                };
                let edge = edge.when(closes, |el| el.pb_3().border_b_1().rounded_b_lg());
                (edge.child(content).into_any_element(), above)
            }
        };
        spaced(div().px_6(), above)
            .when(place.last, |el| el.pb_6())
            .child(content)
            .into_any_element()
    }
}

/// A tile's stand-in picture: a stack that failed or was never stacked.
fn tile_placeholder(icon: IconName, text: String, color: gpui::Hsla) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .text_xs()
        .text_color(color)
        .child(Icon::new(icon).small())
        .child(text)
        .into_any_element()
}

/// The menu of several selected stacks.
fn several_menu(
    menu: PopupMenu,
    this: gpui::WeakEntity<Library>,
    stacks: Vec<StackId>,
    can_delete: bool,
    w: &DesktopWords,
) -> PopupMenu {
    let label = say!(w, "library-menu-delete-stacks", n = stacks.len());
    menu.item(
        entity_item(label, &this, move |l, window, cx| l.delete_stacks(stacks.clone(), window, cx))
            .disabled(!can_delete),
    )
}

/// One stack's menu.
fn tile_menu(
    menu: PopupMenu,
    this: gpui::WeakEntity<Library>,
    renders: Entity<Renders>,
    tile: &Tile,
    cx: &gpui::App,
) -> PopupMenu {
    let w = cx.wording();
    let stack = tile.stack.clone();
    let open = |label: String| {
        let stack = stack.clone();
        entity_item(label, &this, move |l, _, cx| l.open_stack(&stack, cx))
    };
    let rename = {
        let stack = stack.clone();
        entity_item(say!(w, "library-menu-rename"), &this, move |l, window, cx| {
            l.rename_stack(stack.clone(), window, cx)
        })
    };
    let show_frames = {
        let stack = stack.clone();
        entity_item(super::show_frames_label(w), &this, move |l, _, cx| l.show_frames(&stack, cx))
    };
    let can_delete = renders.read(cx).can_delete();
    let delete = |label: String| {
        let stack = stack.clone();
        entity_item(label, &this, move |l, window, cx| {
            l.delete_stacks(vec![stack.clone()], window, cx)
        })
        .disabled(!can_delete)
    };
    let (cover, total) = match &tile.kind {
        Kind::Failed { .. } => {
            return menu
                .item(open(say!(w, "library-menu-open-to-retry")))
                .separator()
                .item(show_frames)
                .separator()
                .item(delete(say!(w, "library-menu-delete-failed")));
        }
        Kind::Unstacked { .. } => {
            return menu
                .item(open(say!(w, "library-menu-open")))
                .separator()
                .item(rename)
                .separator()
                .item(show_frames)
                .separator()
                .item(delete(say!(w, "library-menu-delete-stack")));
        }
        Kind::Renders { cover, total, .. } => (cover.clone(), *total),
    };
    let store = renders.read(cx);
    let (number, picked) = store.find(&cover).map_or((0, false), |r| (r.number, r.picked));
    // In a stack of several, the star and the export say which render.
    let number = number.to_string();
    let star_label = match (picked, total) {
        (false, 1) => say!(w, "render-star"),
        (true, 1) => say!(w, "render-unstar"),
        (false, _) => say!(w, "library-menu-star-number", number = number.as_str()),
        (true, _) => say!(w, "library-menu-unstar-number", number = number.as_str()),
    };
    let export_label = match total {
        1 => say!(w, "library-menu-export"),
        _ => say!(w, "library-menu-export-number", number = number.as_str()),
    };
    // Keep Only Starred: the cleanup once the result wanted is starred.
    let unstarred: Vec<RenderId> =
        stacks::unstarred_of(&stack, store.renders()).into_iter().map(|r| r.id.clone()).collect();
    let can_keep = !unstarred.is_empty() && store.can_delete();
    let weak = renders.downgrade();
    let star = {
        let id = cover.clone();
        entity_item(star_label, &weak, move |r, window, cx| r.toggle_pick(&id, window, cx))
    };
    let export = {
        let id = cover.clone();
        entity_item(export_label, &this, move |_, _, cx| cx.emit(LibraryEvent::Export(id.clone())))
    };
    let keep = {
        let label = match unstarred.len() {
            n if can_keep => say!(w, "library-menu-keep-starred-delete", n = n),
            _ => say!(w, "library-menu-keep-starred"),
        };
        entity_item(label, &weak, move |r, window, cx| {
            if r.can_delete() {
                r.delete(unstarred.clone(), window, cx)
            }
        })
        .disabled(!can_keep)
    };
    menu.item(open(say!(w, "library-menu-open")))
        .separator()
        .item(star)
        .item(export)
        .item(rename)
        .separator()
        .item(show_frames)
        .separator()
        .item(keep)
        .item(delete(say!(w, "library-menu-delete-stack")))
}

impl Render for Library {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Read once: the bar, selection, details and rows go by it.
        let grid = self.grid(cx);
        let today = Local::now().date_naive();
        let left = self.render_left_bar(&grid, cx);
        // Recently Deleted replaces the grid, without details or preview.
        if self.is_showing_deleted() {
            let deleted = self.render_deleted(cx);
            return h_flex()
                .size_full()
                .relative()
                .child(left)
                .child(div().flex_1().min_w_0().h_full().child(deleted));
        }
        // The details pane lists the one selected stack's renders.
        let details_ids = match &self.selected_in(&grid)[..] {
            [stack] if self.details_shown => {
                let renders = self.renders.read(cx).renders();
                renders.iter().filter(|r| r.stack == *stack).map(|r| r.id.clone()).collect()
            }
            _ => Vec::new(),
        };
        let preview = self.render_preview(&grid, cx);
        let muted = cx.theme().muted_foreground;
        let empty = grid.sections.is_empty() && grid.unstacked.is_empty();
        let empty_text = self.empty_text(cx);
        let details = self.details_shown.then(|| self.render_details(&grid, cx));
        let rows = Rc::new(grid.rows(&self.collapsed, self.across()));
        debug_assert!(
            rows.iter().map(Row::key).eq(self.rows.iter().cloned()),
            "the list's rows follow the grid as it changes"
        );
        // The list builds only the rows it draws, and says which.
        let drawn: Rc<RefCell<Vec<usize>>> = Rc::default();
        self.want_thumbnails(rows.clone(), drawn.clone(), details_ids, cx);
        let row = cx.processor(move |this, ix: usize, _, cx| {
            drawn.borrow_mut().push(ix);
            match rows.get(ix) {
                Some(row) => this.render_row(row, today, cx),
                None => div().into_any_element(),
            }
        });
        let grid = div()
            .id("library-grid")
            .debug_selector(|| "library-grid".into())
            .size_full()
            // Tiles and batch headers keep their clicks.
            .on_click(cx.listener(|this, _, _, cx| this.deselect(cx)))
            .map(|el| match empty {
                true => el
                    .px_6()
                    .pt_4()
                    .child(div().pt_8().text_sm().text_color(muted).child(empty_text)),
                false => el.child(list(self.list.clone(), row).size_full().pt_4()),
            });
        h_flex()
            .size_full()
            .relative()
            .child(left)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .relative()
                    .child(grid)
                    .scrollbar(&self.list, ScrollbarAxis::Vertical),
            )
            .children(details)
            .children(preview)
    }
}
