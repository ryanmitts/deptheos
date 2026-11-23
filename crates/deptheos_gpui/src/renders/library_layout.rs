//! The Library's grid model, its list rows, and wording helpers.

use std::path::Path;

use chrono::NaiveDate;
use deptheos_app::library;
use deptheos_app::library::history;

use super::*;
use crate::folders::short_dir;

/// What a tile is.
#[derive(Clone)]
pub(super) enum Kind {
    /// A stack with `total` renders; `cover` is its newest starred render
    /// the filter lets through, else its newest.
    Renders { cover: RenderId, total: usize },
    /// A batch stack that didn't finish, with no render.
    Failed { failure: RenderId },
    /// Frames and settings, never stacked.
    Unstacked { frames: usize, folder: String },
}

/// A tile, read out of the history before drawing: one stack.
#[derive(Clone)]
pub(super) struct Tile {
    pub(super) stack: StackId,
    /// The stack's name.
    pub(super) title: String,
    /// Over the Space preview: "bee #4 ★", or a failed or unstacked stack's name.
    pub(super) preview_title: String,
    pub(super) detail: String,
    /// Its count badge ([`library::Tile::badge`]).
    pub(super) badge: Option<String>,
    pub(super) thumbnail: Option<Arc<RenderImage>>,
    /// Its working version has edits not stacked.
    pub(super) edited: bool,
    pub(super) kind: Kind,
}

/// What a day shows, in order: stacks on their own, and batch runs.
pub(super) enum Run {
    Tiles(Vec<Tile>),
    Batch { batch: Batch, tiles: Vec<Tile>, started: SystemTime, failed: usize },
}

impl Run {
    pub(super) fn tiles(&self) -> &[Tile] {
        match self {
            Self::Tiles(tiles) | Self::Batch { tiles, .. } => tiles,
        }
    }
}

pub(super) struct Section {
    pub(super) day: NaiveDate,
    /// How many stacks.
    pub(super) count: usize,
    pub(super) runs: Vec<Run>,
}

/// What the grid shows, read from the history once per frame: unstacked
/// stacks, then days, with Jump To's spans, counts and failed stacks.
pub(super) struct Grid {
    pub(super) unstacked: Vec<Tile>,
    pub(super) sections: Vec<Section>,
    pub(super) spans: [library::SpanCount; 4],
    pub(super) counts: library::Counts,
    pub(super) failed: Vec<StackId>,
}

impl Grid {
    /// Stacks in drawn order, less those folded batches hide: what ⇧-click
    /// and ⌘A go by.
    pub(super) fn order(&self, collapsed: &HashSet<String>) -> Vec<StackId> {
        let listed = self.sections.iter().flat_map(|s| {
            s.runs.iter().flat_map(|run| match run {
                Run::Batch { batch, .. } if collapsed.contains(&batch.id) => &[][..],
                run => run.tiles(),
            })
        });
        self.unstacked.iter().chain(listed).map(|t| t.stack.clone()).collect()
    }

    /// `stack`'s tile, if it is shown.
    pub(super) fn tile_of(&self, stack: &StackId) -> Option<&Tile> {
        let listed = self.sections.iter().flat_map(|s| s.runs.iter().flat_map(|r| r.tiles()));
        listed.chain(&self.unstacked).find(|t| t.stack == *stack)
    }

    /// The grid as list rows: unstacked stacks under their heading, then each
    /// day with its tiles `across` a row and its batch runs as a header and
    /// rows, unless folded.
    pub(super) fn rows(self, collapsed: &HashSet<String>, across: Across) -> Vec<Row> {
        let mut rows = Vec::new();
        if !self.unstacked.is_empty() {
            rows.push(Row::new(Item::Unstacked { count: self.unstacked.len() }, Above::None));
            push_tiles(&mut rows, self.unstacked, across.loose, false);
            end_section(&mut rows);
        }
        for section in self.sections {
            let day = Item::Day { day: section.day, count: section.count };
            rows.push(Row::new(day, Above::None));
            for run in section.runs {
                match run {
                    Run::Tiles(tiles) => push_tiles(&mut rows, tiles, across.loose, false),
                    Run::Batch { batch, tiles, started, failed } => {
                        let folded = collapsed.contains(&batch.id);
                        let header = BatchHeader {
                            stacks: tiles.iter().map(|t| t.stack.clone()).collect(),
                            failures: tiles
                                .iter()
                                .filter(|t| matches!(t.kind, Kind::Failed { .. }))
                                .map(|t| t.stack.clone())
                                .collect(),
                            peeks: tiles
                                .iter()
                                .filter_map(|t| t.thumbnail.clone())
                                .take(3)
                                .collect(),
                            batch,
                            started,
                            failed,
                            collapsed: folded,
                        };
                        let mut row = Row::new(Item::Batch(header), Above::Run);
                        row.place.boxed = Some(folded);
                        rows.push(row);
                        if !folded {
                            push_tiles(&mut rows, tiles, across.boxed, true);
                        }
                    }
                }
            }
            end_section(&mut rows);
        }
        rows
    }
}

/// The gap between tiles, across and down (a tile's tint reaches into it).
const TILE_GAP: f32 = 16.0;
/// The left bar's width.
pub(super) const LEFT_BAR_WIDTH: f32 = 200.0;
/// The grid's margin either side of its rows.
const GRID_MARGIN: f32 = 24.0;
/// A batch's box either side of its tiles: its padding and its border.
const BATCH_INSET: f32 = 12.0 + 1.0;

/// How many tiles a row holds: loose in a day, and in a batch's box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Across {
    pub(super) loose: usize,
    pub(super) boxed: usize,
}

impl Across {
    /// Tiles that fit in `window` beside the bar (and pane); at least one.
    pub(super) fn of(window: Pixels, details: bool) -> Self {
        let pane = if details { DETAILS_WIDTH } else { 0.0 };
        let grid = f32::from(window) - LEFT_BAR_WIDTH - pane - 2.0 * GRID_MARGIN;
        let fit =
            |width: f32| (((width + TILE_GAP) / (TILE_WIDTH + TILE_GAP)).floor() as usize).max(1);
        Self { loose: fit(grid), boxed: fit(grid - 2.0 * BATCH_INSET) }
    }
}

/// One row of the Library's list, with its place among the rows.
pub(super) struct Row {
    pub(super) item: Item,
    pub(super) place: Place,
}

/// What a row shows.
pub(super) enum Item {
    /// The heading over the stacks not stacked yet: how many there are.
    Unstacked { count: usize },
    /// A day's heading: how many stacks it has.
    Day { day: NaiveDate, count: usize },
    /// A batch run's header, the top of its box.
    Batch(BatchHeader),
    /// As many tiles as fit across.
    Tiles(Vec<Tile>),
}

/// A batch run's header: label, count and date, and its menu's stacks.
pub(super) struct BatchHeader {
    pub(super) batch: Batch,
    pub(super) started: SystemTime,
    pub(super) failed: usize,
    /// Its stacks, and those that failed: what its menu deletes.
    pub(super) stacks: Vec<StackId>,
    pub(super) failures: Vec<StackId>,
    /// The first few covers, shown in the header while it is folded.
    pub(super) peeks: Vec<Arc<RenderImage>>,
    pub(super) collapsed: bool,
}

/// Where a row sits: its spacing and the edges of what it is part of;
/// everything but its item that sets its height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Place {
    pub(super) above: Above,
    /// Inside a batch's box: whether it is the last row, which closes it.
    pub(super) boxed: Option<bool>,
    /// The last row of its day: the gap before the next day under it.
    pub(super) last: bool,
}

/// The space above a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Above {
    /// A heading's.
    None,
    /// A run's first row's, after the heading or the run before.
    Run,
    /// A row of tiles after another.
    Tiles,
}

impl Row {
    fn new(item: Item, above: Above) -> Self {
        Self { item, place: Place { above, boxed: None, last: false } }
    }

    /// The list's key for the row; equal keys mean equal heights.
    pub(super) fn key(&self) -> RowKey {
        let item = match &self.item {
            Item::Unstacked { .. } => ItemKey::Unstacked,
            Item::Day { day, .. } => ItemKey::Day(*day),
            Item::Batch(header) => ItemKey::Batch(header.batch.id.clone()),
            Item::Tiles(tiles) => ItemKey::Tiles(tiles.iter().map(|t| t.stack.clone()).collect()),
        };
        RowKey { item, place: self.place }
    }

    /// The covers of its tiles.
    pub(super) fn covers(&self) -> impl Iterator<Item = &RenderId> {
        let tiles = match &self.item {
            Item::Tiles(tiles) => &tiles[..],
            _ => &[],
        };
        tiles.iter().filter_map(|t| match &t.kind {
            Kind::Renders { cover, .. } => Some(cover),
            _ => None,
        })
    }
}

/// A row as the list's state keeps it ([`Row::key`]).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RowKey {
    pub(super) item: ItemKey,
    pub(super) place: Place,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ItemKey {
    Unstacked,
    Day(NaiveDate),
    Batch(String),
    Tiles(Vec<StackId>),
}

impl RowKey {
    /// The stacks of a row of tiles.
    pub(super) fn stacks(&self) -> Option<&[StackId]> {
        match &self.item {
            ItemKey::Tiles(stacks) => Some(stacks),
            _ => None,
        }
    }

    /// This row stands where `old` did: the same heading, or a tile row
    /// holding `old`'s first.
    pub(super) fn replaces(&self, old: &RowKey) -> bool {
        match (&self.item, &old.item) {
            (ItemKey::Tiles(new), ItemKey::Tiles(old)) => {
                old.first().is_some_and(|s| new.contains(s))
            }
            (new, old) => new == old,
        }
    }
}

/// `tiles` as rows of `across`, boxed when `boxed`: the first spaced as a
/// run is, the rest as tile rows.
fn push_tiles(rows: &mut Vec<Row>, tiles: Vec<Tile>, across: usize, boxed: bool) {
    let mut tiles = tiles.into_iter();
    let mut above = Above::Run;
    loop {
        let row: Vec<Tile> = tiles.by_ref().take(across).collect();
        if row.is_empty() {
            break;
        }
        let mut row = Row::new(Item::Tiles(row), above);
        row.place.boxed = boxed.then_some(tiles.len() == 0);
        rows.push(row);
        above = Above::Tiles;
    }
}

/// The last row pushed ends its day.
fn end_section(rows: &mut [Row]) {
    if let Some(row) = rows.last_mut() {
        row.place.last = true;
    }
}

/// The rows whose covers to read: `drawn` in order, then as many again
/// below and above.
pub(super) fn rows_wanted(drawn: &[usize], len: usize) -> Vec<usize> {
    let mut drawn: Vec<usize> = drawn.iter().copied().filter(|&ix| ix < len).collect();
    drawn.sort_unstable();
    drawn.dedup();
    let (Some(&first), Some(&last)) = (drawn.first(), drawn.last()) else { return drawn };
    let margin = last - first + 1;
    let below = last + 1..(last + 1 + margin).min(len);
    let above = first.saturating_sub(margin)..first;
    drawn.into_iter().chain(below).chain(above).collect()
}

/// An unstacked stack in the grid: one kept, or the one open in Stacking.
pub(super) struct Unstacked {
    stack: StackId,
    name: String,
    /// It has a name of its own (given, or its folder's).
    named: bool,
    frames: usize,
    first: Option<PathBuf>,
}

impl Library {
    /// Stacks with frames and no render: kept ones and the one open.
    fn unstacked_now(&self, renders: &Renders, w: &DesktopWords) -> Vec<Unstacked> {
        let kept = renders
            .unstacked()
            .into_iter()
            .filter(|(stack, _)| self.current.stack.as_ref() != Some(*stack));
        let mut stacks: Vec<Unstacked> = kept
            .filter_map(|(stack, info)| {
                let list = &info.working.as_ref()?.list;
                let name = info.unstacked_name();
                Some(Unstacked {
                    stack: stack.clone(),
                    named: name.is_some(),
                    name: super::super::untitled(name, w),
                    frames: list.shots().len(),
                    first: list.files.first().cloned(),
                })
            })
            .collect();
        if let (Some(stack), Some(now)) = (&self.current.stack, &self.current.unstacked) {
            stacks.push(Unstacked {
                stack: stack.clone(),
                name: now.name.clone(),
                named: now.named,
                frames: now.frames,
                first: now.first.clone(),
            });
        }
        stacks
    }

    /// The Library under the filter and search ([`library::tiles`]), with the
    /// unstacked stacks it indexes.
    fn shared(&self, cx: &gpui::App) -> (library::Tiles, Vec<Unstacked>) {
        let renders = self.renders.read(cx);
        let w = cx.wording();
        let names = renders.stack_names(w);
        let unstacked = self.unstacked_now(renders, w);
        let history = library::History {
            renders: renders
                .renders()
                .iter()
                .map(|r| library::RenderEntry {
                    id: &r.id,
                    stack: &r.stack,
                    stack_name: names.get(&r.stack).map_or("", String::as_str),
                    name: r.name.as_deref(),
                    picked: r.picked,
                    created: r.created,
                    day: library::local_day(r.created),
                    batch: r.batch.as_ref(),
                })
                .collect(),
            failures: renders
                .failures()
                .iter()
                .map(|f| library::FailureEntry {
                    id: &f.id,
                    stack: &f.stack,
                    label: &f.label,
                    created: f.created,
                    day: library::local_day(f.created),
                    batch: &f.batch,
                })
                .collect(),
            unstacked: unstacked
                .iter()
                .map(|u| library::UnstackedEntry { stack: &u.stack, name: &u.name, named: u.named })
                .collect(),
        };
        let tiles =
            library::tiles(&history, self.page.filter(), &self.query, library::today(), w.shared());
        (tiles, unstacked)
    }

    /// The whole Library's counts, whatever the filter shows.
    pub(crate) fn counts(&self, cx: &gpui::App) -> library::Counts {
        self.shared(cx).0.counts
    }

    /// The history's stacks: rendered, failed and unstacked.
    pub(crate) fn stack_count(&self, cx: &gpui::App) -> usize {
        self.counts(cx).stacks
    }

    /// The names of the stacks shown as not stacked yet.
    #[cfg(test)]
    pub fn unstacked_names(&self, cx: &gpui::App) -> Vec<String> {
        self.grid(cx).unstacked.into_iter().map(|u| u.title).collect()
    }

    /// Open the stack not stacked yet named `name`, as a double-click does.
    #[cfg(test)]
    pub fn open_unstacked(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(u) = self.grid(cx).unstacked.into_iter().find(|u| u.title == name) {
            cx.emit(LibraryEvent::OpenStack(u.stack));
        }
    }

    /// The tiles' titles, in the grid's order.
    #[cfg(test)]
    pub fn shown_titles(&self, cx: &gpui::App) -> Vec<String> {
        let sections = self.grid(cx).sections;
        let tiles = sections.iter().flat_map(|s| s.runs.iter().flat_map(|r| r.tiles().to_vec()));
        tiles.map(|t| t.title).collect()
    }

    /// Each tile's count badge by title, in grid order: "2", or "1 of 3"
    /// under a filter.
    #[cfg(test)]
    pub fn shown_badges(&self, cx: &gpui::App) -> Vec<(String, Option<String>)> {
        let sections = self.grid(cx).sections;
        let tiles = sections.iter().flat_map(|s| s.runs.iter().flat_map(|r| r.tiles().to_vec()));
        tiles.map(|t| (t.title.clone(), t.badge.clone())).collect()
    }

    /// The grid's stacks in the order it draws them ([`Grid::order`]).
    pub(super) fn order(&self, cx: &gpui::App) -> Vec<StackId> {
        self.grid(cx).order(&self.collapsed)
    }

    /// `stack`'s tile, if the grid shows it.
    pub(super) fn tile_of(&self, stack: &StackId, cx: &gpui::App) -> Option<Tile> {
        self.grid(cx).tile_of(stack).cloned()
    }

    /// What the renders take on disk, "4.2 GB", once measured.
    pub(super) fn total_size(&self, cx: &gpui::App) -> Option<String> {
        let renders = self.renders.read(cx);
        renders.size_text(renders.renders().iter().map(|r| &r.id), cx.wording())
    }

    /// The title bar's count of what the filter shows: "12 stacks · 68
    /// renders · 4.2 GB", "5 starred renders in 3 stacks".
    pub fn count_text(&self, cx: &gpui::App) -> String {
        let size = self.total_size(cx);
        self.counts(cx).line(self.page.filter(), size.as_deref(), cx.wording().shared())
    }

    /// `stack`'s working version has unstacked edits (open or kept);
    /// `decoder_choice` is looked up once for all tiles.
    pub(super) fn is_edited(
        &self,
        renders: &Renders,
        stack: &StackId,
        decoder_choice: bool,
    ) -> bool {
        if self.current.stack.as_ref() == Some(stack) {
            return self.current.edited.is_some();
        }
        let kept = renders.stack_info(stack).and_then(|i| i.working.as_ref());
        kept.is_some_and(|w| {
            let base = w.base.as_ref().and_then(|b| renders.find(b));
            base.is_some_and(|base| w.changes(base, kept_compared(w, decoder_choice)).any())
        })
    }

    /// The grid now: the engine's tiles worded, with thumbnails, unstacked
    /// first, then the days.
    pub(super) fn grid(&self, cx: &gpui::App) -> Grid {
        let (shared, unstacked) = self.shared(cx);
        let renders = self.renders.read(cx);
        let names = renders.stack_names(cx.wording());
        let name_of = |stack: &StackId| names.get(stack).cloned().unwrap_or_default();
        let decoder_choice = self.decoder_choice;
        let w = cx.wording();
        let tile = |t: &library::Tile| -> Tile {
            let library::Tile { stack, kind } = t;
            let badge = t.badge(w.shared());
            match kind {
                library::TileKind::Renders { cover, total, .. } => {
                    let cover = renders.find(cover).expect("a tile's cover is in the history");
                    let settings = cover.config.summary(w.shared());
                    // The number says which render the tile shows, so an only
                    // render goes without; the star follows the number.
                    let number = history::render_name(w.shared(), cover.number, None, cover.picked);
                    let detail = match (total, cover.picked) {
                        (1, false) => settings,
                        _ => say!(
                            w,
                            "library-tile-detail",
                            render = number.as_str(),
                            settings = settings
                        ),
                    };
                    let title = name_of(stack);
                    Tile {
                        preview_title: say!(
                            w,
                            "render-title",
                            stack = title.as_str(),
                            render = number
                        ),
                        title,
                        detail,
                        badge,
                        thumbnail: renders.thumbnail(&cover.id),
                        edited: self.is_edited(renders, stack, decoder_choice),
                        kind: Kind::Renders { cover: cover.id.clone(), total: *total },
                        stack: stack.clone(),
                    }
                }
                library::TileKind::Failed { failure } => {
                    let label =
                        renders.failure(failure).map(|f| f.label.clone()).unwrap_or_default();
                    Tile {
                        stack: stack.clone(),
                        preview_title: label.clone(),
                        title: label,
                        detail: say!(w, "library-tile-failed-detail"),
                        badge,
                        thumbnail: None,
                        edited: false,
                        kind: Kind::Failed { failure: failure.clone() },
                    }
                }
            }
        };
        let sections = shared
            .days
            .iter()
            .map(|day| Section {
                day: day.day,
                count: day.stacks(),
                runs: day
                    .runs
                    .iter()
                    .map(|run| match run {
                        library::Run::Loose(tiles) => Run::Tiles(tiles.iter().map(&tile).collect()),
                        library::Run::Batch { batch, tiles, started, failed } => Run::Batch {
                            batch: batch.clone(),
                            tiles: tiles.iter().map(&tile).collect(),
                            started: *started,
                            failed: *failed,
                        },
                    })
                    .collect(),
            })
            .collect();
        let unstacked = shared
            .unstacked
            .iter()
            .map(|&i| {
                let u = &unstacked[i];
                Tile {
                    stack: u.stack.clone(),
                    preview_title: u.name.clone(),
                    title: u.name.clone(),
                    detail: say!(cx, "frames-count", n = u.frames),
                    badge: None,
                    thumbnail: None,
                    edited: false,
                    kind: Kind::Unstacked {
                        frames: u.frames,
                        folder: folder_text(u.first.as_deref()),
                    },
                }
            })
            .collect();
        Grid {
            unstacked,
            sections,
            spans: shared.spans,
            counts: shared.counts,
            failed: shared.failed_shown(),
        }
    }

    /// The frame Show Frames in Finder shows: the first of the cover's, a
    /// failed stack's, or a never-stacked working version's.
    pub(crate) fn first_frame(&self, stack: &StackId, cx: &gpui::App) -> Option<PathBuf> {
        let renders = self.renders.read(cx);
        match self.tile_of(stack, cx)?.kind {
            Kind::Renders { cover, .. } => renders.find(&cover)?.frames.first().cloned(),
            Kind::Failed { failure } => renders.failure(&failure)?.frames.first().cloned(),
            Kind::Unstacked { .. } => match (&self.current.stack, &self.current.unstacked) {
                (Some(open), Some(now)) if open == stack => now.first.clone(),
                _ => renders.stack_info(stack)?.working.as_ref()?.list.files.first().cloned(),
            },
        }
    }

    /// `stack`'s working version when worth saying: open with edits, or kept.
    pub(super) fn kept_text(
        &self,
        w: &DesktopWords,
        renders: &Renders,
        stack: &StackId,
    ) -> Option<(String, String)> {
        if self.current.stack.as_ref() == Some(stack) {
            let edited = self.current.edited.clone()?;
            return Some((
                say!(w, "library-working"),
                say!(w, "library-working-edited", edited = edited),
            ));
        }
        let working = renders.stack_info(stack)?.working.as_ref()?;
        let base = working.base.as_ref().and_then(|b| renders.find(b));
        let text = match base {
            // As the title and inspector say it: "Edited since #4", "2
            // changes since #4".
            Some(base) => match working
                .changes(base, kept_compared(working, self.decoder_choice))
                .since(base.number, w.shared())
            {
                Some(changes) => say!(w, "library-working-edited", edited = changes),
                None => say!(w, "library-working-started", number = base.number.to_string()),
            },
            None => say!(w, "library-working-never-stacked"),
        };
        Some((say!(w, "library-working-kept"), text))
    }
}

/// The stack an arrow moves to from `from`: the one before or after, or the
/// same column above or below (a shorter row's last). Stays at an edge; from
/// nothing, the first.
pub(super) fn stepped<T: Clone + PartialEq>(
    rows: &[Vec<T>],
    from: Option<&T>,
    step: Step,
) -> Option<T> {
    let first = rows.iter().flatten().next().cloned();
    let Some(from) = from else { return first };
    let Some((r, c)) =
        rows.iter().enumerate().find_map(|(r, row)| Some((r, row.iter().position(|s| s == from)?)))
    else {
        return first;
    };
    let flat: Vec<&T> = rows.iter().flatten().collect();
    let at = rows[..r].iter().map(Vec::len).sum::<usize>() + c;
    let to = match step {
        Step::Previous => at.checked_sub(1).and_then(|i| flat.get(i)).copied(),
        Step::Next => flat.get(at + 1).copied(),
        Step::Up => r.checked_sub(1).and_then(|r| rows[r].get(c).or(rows[r].last())),
        Step::Down => rows.get(r + 1).and_then(|row| row.get(c).or(row.last())),
    };
    Some(to.unwrap_or(from).clone())
}

/// The end arrows move from: the one farthest from the anchor.
pub(super) fn head_of(order: &[StackId], stacks: &[StackId], anchor: &StackId) -> Option<StackId> {
    let at = |s: &StackId| order.iter().position(|o| o == s);
    let a = at(anchor)?;
    let picked: Vec<usize> = stacks.iter().filter_map(at).collect();
    let (min, max) = (picked.iter().copied().min()?, picked.iter().copied().max()?);
    Some(order[if max > a { max } else { min }].clone())
}

/// Where a picture of size `own` goes in `room`: as large as fits, never
/// past its own size, centered.
pub(super) fn fit_within(own: gpui::Size<Pixels>, room: Bounds<Pixels>) -> Bounds<Pixels> {
    if own.width <= px(0.0) || own.height <= px(0.0) {
        return Bounds { origin: room.center(), size: gpui::size(px(0.0), px(0.0)) };
    }
    let scale = (room.size.width / own.width).min(room.size.height / own.height).clamp(0.0, 1.0);
    Bounds::centered_at(room.center(), gpui::size(own.width * scale, own.height * scale))
}

/// "No renders", "1 render", "3 renders", with "· 610 MB" when `size` is known.
pub(super) fn renders_text(w: &DesktopWords, n: usize, size: Option<String>) -> String {
    match size {
        Some(size) => say!(w, "library-details-renders-size", n = n, size = size),
        None => say!(w, "library-details-renders", n = n),
    }
}

/// "46", or "44 of 46" when some were left out.
pub(super) fn frames_text(w: &DesktopWords, stacked: usize, total: usize) -> String {
    match stacked == total {
        true => total.to_string(),
        false => say!(w, "library-details-frames-of", stacked = stacked, total = total),
    }
}

/// A frame's folder, with the home folder as "~" ([`short_dir`]).
pub(super) fn folder_text(frame: Option<&Path>) -> String {
    frame.and_then(Path::parent).map(short_dir).unwrap_or_default()
}

/// What counts as a change to a kept working version: what would count on
/// screen.
pub(super) fn kept_compared(
    working: &deptheos_app::library::stacks::Working,
    decoder_choice: bool,
) -> deptheos_app::library::stacks::Compared {
    crate::params::compared_settings(&working.list, decoder_choice)
}

/// The settings offer a RAW decoder choice (the converter is installed);
/// looks on disk.
pub(super) fn decoder_choice() -> bool {
    crate::params::installed_backends().len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Five stacks three to a row, then two.
    fn rows() -> Vec<Vec<&'static str>> {
        vec![vec!["a", "b", "c"], vec!["d", "e"]]
    }

    #[test]
    fn a_preview_fits_whole_and_never_grows() {
        let room = Bounds::new(gpui::point(px(0.0), px(0.0)), gpui::size(px(800.0), px(400.0)));
        // Larger than the room: as large as fits whole, centered.
        let fitted = fit_within(gpui::size(px(512.0), px(512.0)), room);
        assert_eq!(fitted.size, gpui::size(px(400.0), px(400.0)));
        assert_eq!(fitted.origin, gpui::point(px(200.0), px(0.0)));
        // Smaller: at its own size, centered.
        let own = fit_within(gpui::size(px(192.0), px(128.0)), room);
        assert_eq!(own.size, gpui::size(px(192.0), px(128.0)));
        assert_eq!(own.center(), room.center());
        // Wide: by its width.
        let wide = fit_within(gpui::size(px(1600.0), px(400.0)), room);
        assert_eq!(wide.size, gpui::size(px(800.0), px(200.0)));
    }

    /// Tiles per row fit beside the bar and pane, fewer in a batch box, at
    /// least one.
    #[test]
    fn a_row_holds_what_fits_across() {
        // 1280 - 200 - 272 - 48 = 760 px: 4 tiles of 140 with 3 gaps of 16.
        assert_eq!(Across::of(px(1280.0), true), Across { loose: 4, boxed: 4 });
        // 608 px: 4 tiles take 608; in a batch's box, 582 px, 3.
        assert_eq!(Across::of(px(1128.0), true), Across { loose: 4, boxed: 3 });
        assert_eq!(Across::of(px(1280.0), false), Across { loose: 6, boxed: 6 });
        assert_eq!(Across::of(px(300.0), true), Across { loose: 1, boxed: 1 });
    }

    /// Of a long Library, only the drawn rows' covers and as many below and
    /// above are read.
    #[test]
    fn the_rows_wanted_are_those_drawn_and_their_neighbors() {
        // Rows 10 to 13 drawn, the list measuring 12 again in the frame.
        assert_eq!(
            rows_wanted(&[10, 11, 12, 12, 13], 1000),
            [10, 11, 12, 13, 14, 15, 16, 17, 6, 7, 8, 9]
        );
        // At the ends, what there is.
        assert_eq!(rows_wanted(&[0, 1], 3), [0, 1, 2]);
        assert_eq!(rows_wanted(&[], 3), Vec::<usize>::new());
    }

    #[test]
    fn the_arrows_move_along_and_across_the_rows() {
        let rows = rows();
        let step = |from: Option<&'static str>, step| stepped(&rows, from.as_ref(), step);
        assert_eq!(step(None, Step::Down), Some("a"), "from nothing, the first");
        assert_eq!(step(Some("c"), Step::Next), Some("d"), "along the grid, to the next row");
        assert_eq!(step(Some("d"), Step::Previous), Some("c"));
        assert_eq!(step(Some("a"), Step::Previous), Some("a"), "at an edge it stays");
        assert_eq!(step(Some("e"), Step::Next), Some("e"));
        assert_eq!(step(Some("b"), Step::Down), Some("e"), "the same column below");
        assert_eq!(step(Some("c"), Step::Down), Some("e"), "the shorter row's last");
        assert_eq!(step(Some("e"), Step::Up), Some("b"));
        assert_eq!(step(Some("a"), Step::Up), Some("a"));
        assert_eq!(step(Some("d"), Step::Down), Some("d"));
    }
}
