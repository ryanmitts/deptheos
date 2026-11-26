//! The Library's tiles, derived each time from the history (ADR-144): one
//! tile per stack a filter and search let through, by day and batch run,
//! with Jump To's spans and the counts. Nothing is stored; each front end
//! calls [`tiles`] and draws the answer.

use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDate};

use crate::words::{Words, say};
use history::{Batch, RenderId, StackId, StackTile, StripEntry, StripRun};

pub mod export;
pub(crate) mod export_tags;
pub mod history;
pub(crate) mod ids;
pub(crate) mod render_file;
pub mod render_folder;
pub mod stacks;
pub mod thumbnails;

/// Which stacks the Library shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Every stack, failed and not stacked yet included.
    #[default]
    All,
    /// The stacks with a starred render, showing only those.
    Starred,
    /// The batch stacks that failed and have no render.
    Failed,
}

/// Where Jump To goes: the days a tile falls in, counted back from today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Span {
    Today,
    Yesterday,
    /// The seven days before yesterday.
    LastWeek,
    Earlier,
}

impl Span {
    pub const ALL: [Self; 4] = [Self::Today, Self::Yesterday, Self::LastWeek, Self::Earlier];

    /// The span `day` falls in, seen from `today`; a future day is today's.
    pub fn of(day: NaiveDate, today: NaiveDate) -> Self {
        match (today - day).num_days() {
            ..=0 => Self::Today,
            1 => Self::Yesterday,
            2..=7 => Self::LastWeek,
            _ => Self::Earlier,
        }
    }

    /// Its name in Jump To.
    pub fn label(self, words: &Words) -> String {
        match self {
            Self::Today => say!(words, "library-span-today"),
            Self::Yesterday => say!(words, "library-span-yesterday"),
            Self::LastWeek => say!(words, "library-span-last-week"),
            Self::Earlier => say!(words, "library-span-earlier"),
        }
    }
}

/// A render, as the Library sorts it.
#[derive(Clone, Copy, Debug)]
pub struct RenderEntry<'a> {
    pub id: &'a RenderId,
    pub stack: &'a StackId,
    /// Its stack's name as its tile shows it ("Untitled stack" worded).
    pub stack_name: &'a str,
    /// The name the user gave the render.
    pub name: Option<&'a str>,
    pub picked: bool,
    pub created: SystemTime,
    /// The local day it was made ([`local_day`]).
    pub day: NaiveDate,
    pub batch: Option<&'a Batch>,
}

/// A batch stack that failed.
#[derive(Clone, Copy, Debug)]
pub struct FailureEntry<'a> {
    pub id: &'a RenderId,
    pub stack: &'a StackId,
    /// What the run called it.
    pub label: &'a str,
    pub created: SystemTime,
    pub day: NaiveDate,
    pub batch: &'a Batch,
}

/// A stack with frames and no render: one kept, or the one on screen.
#[derive(Clone, Copy, Debug)]
pub struct UnstackedEntry<'a> {
    pub stack: &'a StackId,
    /// Its name as its tile shows it, and whether it has one of its own
    /// (the unnamed sort last).
    pub name: &'a str,
    pub named: bool,
}

/// What the Library is derived from: the history and the stacks not
/// stacked yet, the one on screen among them.
#[derive(Clone, Debug, Default)]
pub struct History<'a> {
    pub renders: Vec<RenderEntry<'a>>,
    pub failures: Vec<FailureEntry<'a>>,
    pub unstacked: Vec<UnstackedEntry<'a>>,
}

/// What a tile is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// A stack with renders. `cover` is the newest starred render let
    /// through, else the newest; `shown` are those let through, newest
    /// first, of `total`.
    Renders { cover: RenderId, shown: Vec<RenderId>, total: usize },
    /// A batch stack that failed, with no render: its newest failure.
    Failed { failure: RenderId },
}

/// One stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub stack: StackId,
    pub kind: TileKind,
}

impl Tile {
    /// Its count badge: "4", or "2 of 4" when filtered; none for a single
    /// render or a failed stack.
    pub fn badge(&self, words: &Words) -> Option<String> {
        match &self.kind {
            TileKind::Renders { shown, total, .. } if shown.len() < *total => {
                let (shown, total) = (shown.len(), *total);
                Some(say!(words, "library-badge-of", shown = shown, total = total))
            }
            TileKind::Renders { total, .. } if *total > 1 => Some(total.to_string()),
            _ => None,
        }
    }
}

/// What a day shows, newest first: single stacks and whole batch runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
    Loose(Vec<Tile>),
    /// The stacks a batch made, in order; `started` is the oldest shown's
    /// time, `failed` how many failed.
    Batch {
        batch: Batch,
        tiles: Vec<Tile>,
        started: SystemTime,
        failed: usize,
    },
}

impl Run {
    pub fn tiles(&self) -> &[Tile] {
        match self {
            Self::Loose(tiles) | Self::Batch { tiles, .. } => tiles,
        }
    }
}

/// A day of tiles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Day {
    pub day: NaiveDate,
    pub runs: Vec<Run>,
}

impl Day {
    /// How many stacks it shows.
    pub fn stacks(&self) -> usize {
        self.runs.iter().map(|r| r.tiles().len()).sum()
    }
}

/// A span of Jump To: its stack count and its first day's index, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanCount {
    pub span: Span,
    pub stacks: usize,
    pub first: Option<usize>,
}

/// The whole Library's counts, whatever the filter or search shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Every stack: with renders, failed, or not stacked yet.
    pub stacks: usize,
    pub renders: usize,
    pub starred_renders: usize,
    /// The stacks with a starred render.
    pub starred_stacks: usize,
    /// The failed stacks with no render.
    pub failed_stacks: usize,
}

impl Counts {
    /// How many stacks `filter` shows, with no search.
    pub fn of(&self, filter: Filter) -> usize {
        match filter {
            Filter::All => self.stacks,
            Filter::Starred => self.starred_stacks,
            Filter::Failed => self.failed_stacks,
        }
    }

    /// The count line under `filter`: "12 stacks · 68 renders · 4.2 GB".
    /// `size` is every render's size on disk, once measured.
    pub fn line(&self, filter: Filter, size: Option<&str>, words: &Words) -> String {
        let (stacks, renders) = (self.stacks, self.renders);
        match (filter, size.filter(|_| renders > 0)) {
            (Filter::All, Some(size)) => {
                say!(words, "library-count-size", stacks = stacks, renders = renders, size = size)
            }
            (Filter::All, None) => say!(words, "library-count", stacks = stacks, renders = renders),
            (Filter::Starred, _) => say!(
                words,
                "library-count-starred",
                renders = self.starred_renders,
                stacks = self.starred_stacks
            ),
            (Filter::Failed, _) => {
                say!(words, "library-count-failed", stacks = self.failed_stacks)
            }
        }
    }
}

/// The Library as one filter and search show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tiles {
    /// The stacks not stacked yet shown, as indexes into
    /// [`History::unstacked`]: under All only, by name.
    pub unstacked: Vec<usize>,
    /// Newest day first.
    pub days: Vec<Day>,
    /// Jump To, every span in order.
    pub spans: [SpanCount; 4],
    pub counts: Counts,
}

impl Tiles {
    /// The failed stacks shown, which Delete Failed Stacks… deletes.
    pub fn failed_shown(&self) -> Vec<StackId> {
        self.tiles()
            .filter(|t| matches!(t.kind, TileKind::Failed { .. }))
            .map(|t| t.stack.clone())
            .collect()
    }

    /// Every tile of the days, in order.
    pub fn tiles(&self) -> impl Iterator<Item = &Tile> {
        self.days.iter().flat_map(|d| d.runs.iter().flat_map(Run::tiles))
    }

    /// Nothing shown: neither a day nor a stack not stacked yet.
    pub fn is_empty(&self) -> bool {
        self.days.is_empty() && self.unstacked.is_empty()
    }
}

/// A render or a failure, by index into the history.
#[derive(Clone, Copy)]
enum Member {
    Render(usize),
    Failure(usize),
}

/// The Library under `filter` and the search `query`, on `today`.
///
/// Lifecycle: none of its own; derived each time from the history, the
/// filter and the search (ADR-144). iOS keeps the last answer in
/// `AppModel.libraryMemo` (`Model/AppModel+Library.swift`).
///
/// - A render is let through on its own (Starred: only starred), searched
///   by its stack's name, its own and its batch's label; its stack shows at
///   the newest, in its batch.
/// - A failure shows only while its stack has no render, and under All only
///   while it has no kept working version (then it shows as not stacked).
/// - The stacks not stacked yet show under All, searched by name.
/// - Every stack is one tile.
pub fn tiles(
    history: &History,
    filter: Filter,
    query: &str,
    today: NaiveDate,
    words: &Words,
) -> Tiles {
    let counts = counts(history);
    let mut totals: HashMap<&StackId, usize> = HashMap::new();
    for r in &history.renders {
        *totals.entry(r.stack).or_default() += 1;
    }
    let unstacked: HashSet<&StackId> = history.unstacked.iter().map(|u| u.stack).collect();
    let renders = history.renders.iter().enumerate().filter(|(_, r)| {
        let wanted = match filter {
            Filter::All => true,
            Filter::Starred => r.picked,
            Filter::Failed => false,
        };
        wanted && {
            let batch = r.batch.map(|b| b.label.text(words));
            history::matches_search(
                &[r.stack_name, r.name.unwrap_or_default()],
                batch.as_deref(),
                query,
            )
        }
    });
    let failures = history.failures.iter().enumerate().filter(|(_, f)| {
        let wanted = match filter {
            Filter::All => !unstacked.contains(f.stack),
            Filter::Starred => false,
            Filter::Failed => true,
        };
        wanted
            && !totals.contains_key(f.stack)
            && history::matches_search(&[f.label], Some(&f.batch.label.text(words)), query)
    });
    let entries: Vec<(StackId, StripEntry<Member, NaiveDate>)> = renders
        .map(|(i, r)| (r.stack, Member::Render(i), r.id, r.day, r.batch))
        .chain(failures.map(|(i, f)| (f.stack, Member::Failure(i), f.id, f.day, Some(f.batch))))
        .map(|(stack, key, id, day, batch)| {
            (stack.clone(), StripEntry { key, id: id.clone(), day, batch: batch.cloned() })
        })
        .collect();
    // Every batch's first day, so a search doesn't move a batch.
    let all = history
        .renders
        .iter()
        .filter_map(|r| Some((r.batch?, r.day)))
        .chain(history.failures.iter().map(|f| (f.batch, f.day)));
    let tile = |StackTile { stack, members }: StackTile<Member>| -> (Tile, SystemTime) {
        let created = members
            .iter()
            .map(|m| match *m {
                Member::Render(i) => history.renders[i].created,
                Member::Failure(i) => history.failures[i].created,
            })
            .min()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        // Members are newest first: the newest starred, else the newest.
        let shown: Vec<&RenderEntry> = members
            .iter()
            .filter_map(|m| match *m {
                Member::Render(i) => Some(&history.renders[i]),
                Member::Failure(_) => None,
            })
            .collect();
        let kind = match shown.iter().find(|r| r.picked).or(shown.first()) {
            Some(cover) => TileKind::Renders {
                cover: cover.id.clone(),
                total: totals.get(&stack).copied().unwrap_or(shown.len()),
                shown: shown.iter().map(|r| r.id.clone()).collect(),
            },
            None => {
                let newest = members.iter().find_map(|m| match *m {
                    Member::Failure(i) => Some(history.failures[i].id.clone()),
                    Member::Render(_) => None,
                });
                TileKind::Failed { failure: newest.expect("a tile has a member") }
            }
        };
        (Tile { stack, kind }, created)
    };
    let days: Vec<Day> = history::strip(entries, all)
        .into_iter()
        .map(|day| {
            let mut runs: Vec<Run> = Vec::new();
            for run in day.runs {
                match run {
                    StripRun::Single(entry) => {
                        let (t, _) = tile(entry);
                        match runs.last_mut() {
                            Some(Run::Loose(tiles)) => tiles.push(t),
                            _ => runs.push(Run::Loose(vec![t])),
                        }
                    }
                    StripRun::Batch(batch, entries) => {
                        let made: Vec<(Tile, SystemTime)> =
                            entries.into_iter().map(&tile).collect();
                        let failed = made
                            .iter()
                            .filter(|(t, _)| matches!(t.kind, TileKind::Failed { .. }))
                            .count();
                        let started =
                            made.iter().map(|(_, at)| *at).min().unwrap_or(SystemTime::UNIX_EPOCH);
                        let tiles = made.into_iter().map(|(t, _)| t).collect();
                        runs.push(Run::Batch { batch, tiles, started, failed });
                    }
                }
            }
            Day { day: day.day, runs }
        })
        .collect();
    let spans = Span::ALL.map(|span| {
        let mut count = SpanCount { span, stacks: 0, first: None };
        for (i, day) in days.iter().enumerate().filter(|(_, d)| Span::of(d.day, today) == span) {
            count.stacks += day.stacks();
            count.first.get_or_insert(i);
        }
        count
    });
    let mut shown_unstacked: Vec<usize> = match filter {
        Filter::All => (0..history.unstacked.len())
            .filter(|&i| history::matches_search(&[history.unstacked[i].name], None, query))
            .collect(),
        Filter::Starred | Filter::Failed => Vec::new(),
    };
    shown_unstacked.sort_by_key(|&i| {
        let u = &history.unstacked[i];
        (!u.named, u.name.to_lowercase(), u.stack.as_str().to_string())
    });
    Tiles { unstacked: shown_unstacked, days, spans, counts }
}

/// The whole Library's counts.
fn counts(history: &History) -> Counts {
    let stacked: HashSet<&StackId> = history.renders.iter().map(|r| r.stack).collect();
    let starred: Vec<&RenderEntry> = history.renders.iter().filter(|r| r.picked).collect();
    let failed: HashSet<&StackId> =
        history.failures.iter().map(|f| f.stack).filter(|s| !stacked.contains(s)).collect();
    let mut stacks = stacked.clone();
    stacks.extend(&failed);
    stacks.extend(history.unstacked.iter().map(|u| u.stack));
    Counts {
        stacks: stacks.len(),
        renders: history.renders.len(),
        starred_renders: starred.len(),
        starred_stacks: starred.iter().map(|r| r.stack).collect::<HashSet<_>>().len(),
        failed_stacks: failed.len(),
    }
}

/// The local day `time` falls on, in the system's time zone.
pub fn local_day(time: SystemTime) -> NaiveDate {
    DateTime::<Local>::from(time).date_naive()
}

/// Today, in the system's time zone.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// When `day` starts in the system's time zone, in Unix seconds, for iOS's
/// calendar.
pub fn day_start(day: NaiveDate) -> i64 {
    let midnight = day.and_hms_opt(0, 0, 0).expect("midnight is a time");
    // A day that starts in a gap (a clock moved forward at midnight)
    // starts at the gap's end, within an hour.
    let local = midnight.and_local_timezone(Local);
    local.earliest().map_or_else(|| midnight.and_utc().timestamp(), |t| t.timestamp())
}

#[cfg(test)]
mod tests;
