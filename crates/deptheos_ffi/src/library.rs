//! The history and the Library: renders, failures, names, the records the
//! engine keeps, the Library's tiles and thumbnails.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use deptheos_app::frames::list::List;
use deptheos_app::library;
use deptheos_app::library::history::{self, Batch, BatchLabel, Failure, Render, RenderId, StackId};
use deptheos_app::library::stacks;
use deptheos_app::runs::batch::Split;

use crate::{BatchPlan, ListInfo, StackSettings, UnstackedInfo, Words, WorkingInfo};

/// A batch run, whose renders the history shows under one header
/// (`deptheos_app::library::history::Batch`).
#[uniffi::remote(Record)]
pub struct Batch {
    pub id: String,
    /// The header, worded by [`Words::batch_label`].
    pub label: BatchLabel,
}

/// What a batch header is worded from
/// (`deptheos_app::library::history::BatchLabel`): how it split its folders
/// and how many frames it skipped.
#[uniffi::remote(Record)]
pub struct BatchLabel {
    pub split: Split,
    pub skipped: u64,
}

/// What a new render is made from, which says what it starts with
/// (`deptheos_app::library::history::Made`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RenderMade {
    /// Stacked from the working version, based on render `base` (none
    /// before the stack's first render), whose adjustments it inherits.
    Stacked { base: Option<String> },
    /// A batch run's stack, which starts neutral.
    Batch { batch: Batch },
}

/// A new run of `plan`, as the Batch sheet planned it.
#[uniffi::export]
pub fn new_batch(plan: BatchPlan) -> Batch {
    Batch::new(BatchLabel { split: plan.split, skipped: u64::from(plan.skipped) })
}

/// A render in the history.
#[derive(Clone, Debug, uniffi::Record)]
pub struct RenderInfo {
    pub id: String,
    /// Milliseconds since 1970.
    pub created_ms: i64,
    /// The name the user gave it.
    pub name: Option<String>,
    /// Its name, else its frames' folder; `None` with neither, which the
    /// app words ("Untitled stack").
    pub title: Option<String>,
    /// The stack it is part of.
    pub stack: String,
    /// Its place in its stack, from 1: "canon1 #4". Numbers never shift.
    pub number: u32,
    /// The stack's given name, else its first render's folder; `None` with
    /// neither, which the app words.
    pub stack_name: Option<String>,
    pub settings: StackSettings,
    pub width: u32,
    pub height: u32,
    /// Every file listed when it ran, in list order; `included` marks the
    /// stacked ones.
    pub frames: Vec<PathBuf>,
    pub included: Vec<bool>,
    /// The batch run that made it, if one did.
    pub batch: Option<Batch>,
    /// Starred.
    pub picked: bool,
}

fn unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

impl RenderInfo {
    /// `render`, in a stack called `stack_name`.
    pub(crate) fn of(render: &Render, stack_name: Option<String>) -> Self {
        Self {
            id: render.id.as_str().to_string(),
            created_ms: unix_ms(render.created),
            name: render.name.clone(),
            title: render.display_name(),
            stack: render.stack.as_str().to_string(),
            number: render.number,
            stack_name,
            settings: StackSettings::of(&render.config, &render.options),
            width: render.width,
            height: render.height,
            frames: render.frames.clone(),
            included: render.included.clone(),
            batch: render.batch.clone(),
            picked: render.picked,
        }
    }
}

/// The records the engine keeps (`Engine::records`) and the ids it couldn't
/// read. The app asks `Engine::showing` as it draws, so neither is reread
/// until the history is relisted, a render saved or frames relinked. An id
/// is kept or missing, never both, and stops being missing only when
/// listed, saved or read.
#[derive(Default)]
pub(crate) struct Records {
    kept: HashMap<RenderId, Render>,
    /// Asked for and not read (deleted, or its file unreadable).
    missing: HashSet<RenderId>,
}

impl Records {
    /// Exactly `renders`, as the history lists them.
    pub(crate) fn listed(renders: &[Render]) -> Self {
        Self {
            kept: renders.iter().map(|r| (r.id.clone(), r.clone())).collect(),
            missing: Default::default(),
        }
    }

    pub(crate) fn get(&self, id: &RenderId) -> Option<&Render> {
        self.kept.get(id)
    }

    /// Asked for and not read since the last listing.
    pub(crate) fn is_missing(&self, id: &RenderId) -> bool {
        self.missing.contains(id)
    }

    /// Just saved or read: kept, and no longer missing.
    pub(crate) fn keep(&mut self, render: &Render) {
        self.missing.remove(&render.id);
        self.kept.insert(render.id.clone(), render.clone());
    }

    /// Deleted or unreadable: missing until listed again.
    pub(crate) fn forget(&mut self, ids: &[RenderId]) {
        for id in ids {
            self.kept.remove(id);
            self.missing.insert(id.clone());
        }
    }

    /// The Library was deleted.
    pub(crate) fn forget_all(&mut self) {
        self.missing.extend(self.kept.drain().map(|(id, _)| id));
    }

    pub(crate) fn of_stack(&self, stack: &StackId) -> Vec<Render> {
        self.kept.values().filter(|r| &r.stack == stack).cloned().collect()
    }
}

/// A failed batch stack, under its run's header: tapping it loads its
/// images and settings to try again.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FailureInfo {
    pub id: String,
    /// Milliseconds since 1970.
    pub created_ms: i64,
    /// What the run called it.
    pub label: String,
    /// In order.
    pub frames: Vec<PathBuf>,
    pub batch: Batch,
    pub error: String,
    pub settings: StackSettings,
    /// The stack it was to be: trying it again joins it.
    pub stack: String,
}

impl From<&Failure> for FailureInfo {
    fn from(failure: &Failure) -> Self {
        Self {
            id: failure.id.as_str().to_string(),
            created_ms: unix_ms(failure.created),
            label: failure.label.clone(),
            frames: failure.frames.clone(),
            batch: failure.batch.clone(),
            error: failure.error.clone(),
            settings: StackSettings::of(&failure.config, &failure.options),
            stack: failure.stack.as_str().to_string(),
        }
    }
}

/// The working version to retry `failure`: its files, all included, and its
/// settings, in its stack, with no base.
#[uniffi::export]
pub fn failure_working(failure: FailureInfo) -> WorkingInfo {
    let list = List::of_frames(&failure.frames, deptheos_app::FrameOrder::Name);
    WorkingInfo {
        stack: failure.stack,
        base: None,
        list: ListInfo::from(&list),
        settings: Some(failure.settings),
    }
}

/// Which stacks the Library shows (`deptheos_app::library::Filter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LibraryFilter {
    All,
    Starred,
    Failed,
}

impl From<LibraryFilter> for library::Filter {
    fn from(filter: LibraryFilter) -> Self {
        match filter {
            LibraryFilter::All => Self::All,
            LibraryFilter::Starred => Self::Starred,
            LibraryFilter::Failed => Self::Failed,
        }
    }
}

/// Where Jump To goes (`deptheos_app::library::Span`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum LibrarySpan {
    Today,
    Yesterday,
    LastWeek,
    Earlier,
}

impl From<library::Span> for LibrarySpan {
    fn from(span: library::Span) -> Self {
        match span {
            library::Span::Today => Self::Today,
            library::Span::Yesterday => Self::Yesterday,
            library::Span::LastWeek => Self::LastWeek,
            library::Span::Earlier => Self::Earlier,
        }
    }
}

/// A render as the Library sorts it: what the tiles need of
/// [`RenderInfo`], so a keystroke doesn't copy every render's frames.
#[derive(Clone, Debug, PartialEq, Eq, Hash, uniffi::Record)]
pub struct LibraryRender {
    pub id: String,
    pub stack: String,
    /// `None` with no name or folder.
    pub stack_name: Option<String>,
    pub name: Option<String>,
    pub picked: bool,
    pub created_ms: i64,
    pub batch: Option<Batch>,
}

/// A failed batch stack, as the Library sorts it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, uniffi::Record)]
pub struct LibraryFailure {
    pub id: String,
    pub stack: String,
    pub label: String,
    pub created_ms: i64,
    pub batch: Batch,
}

/// What a tile is.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LibraryTileKind {
    /// A stack with renders: `cover` is the one shown, `shown` those the
    /// filter and search let through, newest first, of `total`.
    Renders { cover: String, shown: Vec<String>, total: u32 },
    /// A failed batch stack with no render: its newest failure.
    Failed { failure: String },
}

/// One stack, with its count badge ("4", "2 of 4").
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LibraryTile {
    pub stack: String,
    pub kind: LibraryTileKind,
    pub badge: Option<String>,
}

/// What a day shows: stacks made on their own, or a batch run.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LibraryRun {
    Loose {
        tiles: Vec<LibraryTile>,
    },
    /// `started_ms`: when the oldest shown was made; `failed`: how many
    /// failed.
    Batch {
        batch: Batch,
        tiles: Vec<LibraryTile>,
        started_ms: i64,
        failed: u32,
    },
}

/// A day of tiles: `day` is when it starts, in seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LibraryDay {
    pub day: i64,
    pub stacks: u32,
    pub runs: Vec<LibraryRun>,
}

/// A Jump To span: its name, how many stacks the shown days have in it,
/// and the first of those days (an index).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LibrarySpanCount {
    pub span: LibrarySpan,
    pub label: String,
    pub stacks: u32,
    pub first: Option<u32>,
}

/// The Library as one filter and search show it (`deptheos_app::library`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LibraryTiles {
    /// By name, the unnamed last.
    pub unstacked: Vec<UnstackedInfo>,
    /// Newest day first.
    pub days: Vec<LibraryDay>,
    pub spans: Vec<LibrarySpanCount>,
    /// In order.
    pub failed: Vec<String>,
    /// How many stacks each filter shows, with no search: All, Starred,
    /// Failed.
    pub all: u32,
    pub starred: u32,
    pub failed_stacks: u32,
    /// "12 stacks · 68 renders · 4.2 GB", with `size` once measured.
    pub count_line: String,
}

/// What the Library is worked out from: the history's renders and
/// failures, and the unstacked stacks, the one on screen among them if so.
#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryHistory {
    pub renders: Vec<LibraryRender>,
    pub failures: Vec<LibraryFailure>,
    pub unstacked: Vec<UnstackedInfo>,
}

/// The Library under `filter` and `query`, today, as on the desktop
/// (`deptheos_app::library::tiles`). `untitled` names a stack with no name;
/// `size` is every render's size on disk, once measured.
#[uniffi::export]
pub fn library_tiles(
    history: LibraryHistory,
    filter: LibraryFilter,
    query: String,
    untitled: String,
    size: Option<String>,
    words: Arc<Words>,
) -> LibraryTiles {
    let LibraryHistory { renders, failures, unstacked } = history;
    let words = &words.words;
    let time = |ms: i64| UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64);
    let ids = |id: &str, stack: &str| Some((RenderId::parse(id)?, StackId::parse(stack)?));
    let rendered: Vec<_> =
        renders.iter().filter_map(|r| Some((r, ids(&r.id, &r.stack)?))).collect();
    let failed: Vec<_> = failures.iter().filter_map(|f| Some((f, ids(&f.id, &f.stack)?))).collect();
    let kept: Vec<_> =
        unstacked.iter().filter_map(|u| Some((u, StackId::parse(&u.stack)?))).collect();
    let history = library::History {
        renders: rendered
            .iter()
            .map(|(r, (id, stack))| library::RenderEntry {
                id,
                stack,
                stack_name: r.stack_name.as_deref().unwrap_or(&untitled),
                name: r.name.as_deref(),
                picked: r.picked,
                created: time(r.created_ms),
                day: library::local_day(time(r.created_ms)),
                batch: r.batch.as_ref(),
            })
            .collect(),
        failures: failed
            .iter()
            .map(|(f, (id, stack))| library::FailureEntry {
                id,
                stack,
                label: &f.label,
                created: time(f.created_ms),
                day: library::local_day(time(f.created_ms)),
                batch: &f.batch,
            })
            .collect(),
        unstacked: kept
            .iter()
            .map(|(u, stack)| library::UnstackedEntry {
                stack,
                name: u.name.as_deref().unwrap_or(&untitled),
                named: u.name.is_some(),
            })
            .collect(),
    };
    let filter = library::Filter::from(filter);
    let tiles = library::tiles(&history, filter, &query, library::today(), words);
    let tile = |t: &library::Tile| LibraryTile {
        stack: t.stack.as_str().to_string(),
        badge: t.badge(words),
        kind: match &t.kind {
            library::TileKind::Renders { cover, shown, total } => LibraryTileKind::Renders {
                cover: cover.as_str().to_string(),
                shown: shown.iter().map(|id| id.as_str().to_string()).collect(),
                total: *total as u32,
            },
            library::TileKind::Failed { failure } => {
                LibraryTileKind::Failed { failure: failure.as_str().to_string() }
            }
        },
    };
    let counts = tiles.counts;
    LibraryTiles {
        unstacked: tiles.unstacked.iter().map(|&i| kept[i].0.clone()).collect(),
        days: tiles
            .days
            .iter()
            .map(|day| LibraryDay {
                day: library::day_start(day.day),
                stacks: day.stacks() as u32,
                runs: day
                    .runs
                    .iter()
                    .map(|run| match run {
                        library::Run::Loose(tiles) => {
                            LibraryRun::Loose { tiles: tiles.iter().map(tile).collect() }
                        }
                        library::Run::Batch { batch, tiles, started, failed } => {
                            LibraryRun::Batch {
                                batch: batch.clone(),
                                tiles: tiles.iter().map(tile).collect(),
                                started_ms: unix_ms(*started),
                                failed: *failed as u32,
                            }
                        }
                    })
                    .collect(),
            })
            .collect(),
        spans: tiles
            .spans
            .iter()
            .map(|s| LibrarySpanCount {
                span: s.span.into(),
                label: s.span.label(words),
                stacks: s.stacks as u32,
                first: s.first.map(|i| i as u32),
            })
            .collect(),
        failed: tiles.failed_shown().iter().map(|s| s.as_str().to_string()).collect(),
        all: counts.of(library::Filter::All) as u32,
        starred: counts.of(library::Filter::Starred) as u32,
        failed_stacks: counts.of(library::Filter::Failed) as u32,
        count_line: counts.line(filter, size.as_deref(), words),
    }
}

/// What the thumbnail policy says to do (`library::thumbnails::Action`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ThumbnailAction {
    /// Read `ids` one after another, telling [`ThumbnailPolicy::read`] of
    /// each, then [`ThumbnailPolicy::finished`].
    Read { token: u64, ids: Vec<String> },
    /// Keep the pixels just read for `id`; one read and not kept is let go.
    Keep { id: String },
    /// Let go of `id`'s pixels.
    Drop { id: String },
}

/// Which thumbnails to read, keep and let go of, as on the desktop
/// (`deptheos_app::library::thumbnails`, ADR-144). The app holds the pixels
/// and does what each call returns. No call blocks: each is a policy step
/// under a lock no other work takes, at the moment's clock.
#[derive(uniffi::Object)]
pub struct ThumbnailPolicy {
    policy: Mutex<library::thumbnails::Thumbnails>,
}

impl ThumbnailPolicy {
    fn on(&self, event: library::thumbnails::Event) -> Vec<ThumbnailAction> {
        let mut policy = self.policy.lock().unwrap_or_else(|e| e.into_inner());
        let id = |id: RenderId| id.as_str().to_string();
        policy
            .on(event)
            .into_iter()
            .map(|action| match action {
                library::thumbnails::Action::Read { token, ids } => {
                    ThumbnailAction::Read { token: token.0, ids: ids.into_iter().map(id).collect() }
                }
                library::thumbnails::Action::Keep(r) => ThumbnailAction::Keep { id: id(r) },
                library::thumbnails::Action::Drop(r) => ThumbnailAction::Drop { id: id(r) },
            })
            .collect()
    }
}

#[uniffi::export]
impl ThumbnailPolicy {
    /// At most `kept` thumbnails shown at once.
    #[uniffi::constructor]
    pub fn new(kept: u32) -> Arc<Self> {
        let policy = library::thumbnails::Thumbnails::new(kept as usize);
        Arc::new(Self { policy: Mutex::new(policy) })
    }

    /// Those in view, the most wanted first.
    pub fn wanted(&self, ids: Vec<String>) -> Vec<ThumbnailAction> {
        let ids = ids.iter().filter_map(|id| RenderId::parse(id)).collect();
        self.on(library::thumbnails::Event::Wanted { ids, now: Instant::now() })
    }

    /// `id` of read `token` was read, or could not be.
    pub fn read(&self, token: u64, id: String, ok: bool) -> Vec<ThumbnailAction> {
        let Some(id) = RenderId::parse(&id) else { return Vec::new() };
        let token = library::thumbnails::ReadToken(token);
        self.on(library::thumbnails::Event::Read { token, id, ok, now: Instant::now() })
    }

    /// Read `token` is over.
    pub fn finished(&self, token: u64) -> Vec<ThumbnailAction> {
        let token = library::thumbnails::ReadToken(token);
        self.on(library::thumbnails::Event::Finished { token, now: Instant::now() })
    }

    /// These renders were deleted.
    pub fn deleted(&self, ids: Vec<String>) -> Vec<ThumbnailAction> {
        let ids = ids.iter().filter_map(|id| RenderId::parse(id)).collect();
        self.on(library::thumbnails::Event::Deleted { ids })
    }

    /// Memory is short: keep only what is wanted.
    pub fn shed(&self) -> Vec<ThumbnailAction> {
        self.on(library::thumbnails::Event::Shed)
    }

    /// The whole Library was deleted.
    pub fn cleared(&self) -> Vec<ThumbnailAction> {
        self.on(library::thumbnails::Event::Cleared)
    }
}

/// A Recently Deleted record (`deptheos_app::library::history::DeletedItem`):
/// what one delete took of one stack, kept 30 days, restored whole. Worded
/// by [`Words::deleted_title`] and [`Words::deleted_days_left`].
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct DeletedInfo {
    pub id: String,
    /// In milliseconds since 1970.
    pub deleted_ms: i64,
    pub stack: String,
    /// What its stack was called; none for one with no name.
    pub name: Option<String>,
    /// The whole stack went (renders, failed runs, name, working version),
    /// not some renders.
    pub whole: bool,
    /// The numbers of its renders, in order.
    pub numbers: Vec<u32>,
    pub failed: u32,
    /// In bytes.
    pub size: u64,
    /// Newest first: the first's thumbnail stands for it
    /// (`Engine::deleted_thumbnail`).
    pub renders: Vec<String>,
}

/// What [`crate::Engine::delete_renders`] did.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RendersDeleted {
    /// One per stack the renders were of.
    pub records: Vec<DeletedInfo>,
    /// The base now: the one given, or the stack's newest render left once
    /// it was deleted.
    pub base: Option<String>,
}

/// Keep Only Starred: `stack`'s unstarred `renders`, to delete; none when
/// none of the stack's is starred
/// (`deptheos_app::library::stacks::unstarred_of`).
#[uniffi::export]
pub fn unstarred_of(stack: String, renders: Vec<RenderInfo>) -> Vec<String> {
    let of_stack = renders.into_iter().filter(|r| r.stack == stack).collect();
    stacks::unstarred(of_stack, |r: &RenderInfo| r.picked).into_iter().map(|r| r.id).collect()
}

impl From<&history::DeletedItem> for DeletedInfo {
    fn from(item: &history::DeletedItem) -> Self {
        let (whole, numbers, failed) = match &item.took {
            history::Took::Stack { .. } => (true, Vec::new(), 0),
            history::Took::Renders { numbers, failed } => (false, numbers.clone(), *failed as u32),
        };
        Self {
            id: item.id.as_str().to_string(),
            deleted_ms: unix_ms(item.deleted),
            stack: item.stack.as_str().to_string(),
            name: item.name.clone(),
            whole,
            numbers,
            failed,
            size: item.size,
            renders: item.renders.iter().map(|r| r.as_str().to_string()).collect(),
        }
    }
}

impl DeletedInfo {
    /// The shared layer's record, to word; `None` for ids it couldn't have
    /// written.
    pub(crate) fn item(&self) -> Option<history::DeletedItem> {
        let renders = self.renders.iter().map(|r| RenderId::parse(r)).collect::<Option<_>>()?;
        let took = match self.whole {
            true => history::Took::Stack { renders: self.renders.len() },
            false => history::Took::Renders {
                numbers: self.numbers.clone(),
                failed: self.failed as usize,
            },
        };
        Some(history::DeletedItem {
            id: history::DeletedId::parse(&self.id)?,
            deleted: UNIX_EPOCH + std::time::Duration::from_millis(self.deleted_ms.max(0) as u64),
            stack: StackId::parse(&self.stack)?,
            name: self.name.clone(),
            took,
            size: self.size,
            renders,
        })
    }
}
