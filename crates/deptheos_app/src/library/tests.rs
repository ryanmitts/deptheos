use std::time::{Duration, SystemTime};

use chrono::NaiveDate;

use super::*;
use crate::words::explain::Platform;

fn id(n: u64) -> RenderId {
    RenderId::parse(&format!("{n:032x}")).unwrap()
}

fn stack(n: u64) -> StackId {
    StackId::parse(&id(1000 + n).0).unwrap()
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
}

fn at(n: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(n)
}

/// Render n: (n, its stack, the stack's name, its own name, starred,
/// day, in the batch).
type Rendered = (u64, u64, &'static str, Option<&'static str>, bool, u32, bool);

/// The history the tests read: owned, lent to [`History`].
struct Fixture {
    ids: Vec<RenderId>,
    stacks: Vec<StackId>,
    batch: Batch,
    renders: Vec<Rendered>,
    /// Failure n: (n, its stack, label, day).
    failures: Vec<(u64, u64, &'static str, u32)>,
    /// (stack, name, named).
    unstacked: Vec<(u64, &'static str, bool)>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            ids: (0..100).map(id).collect(),
            stacks: (0..20).map(stack).collect(),
            batch: Batch {
                id: "b".into(),
                label: crate::library::history::BatchLabel {
                    split: crate::runs::batch::Split::AutoSort,
                    skipped: 0,
                },
            },
            renders: Vec::new(),
            failures: Vec::new(),
            unstacked: Vec::new(),
        }
    }

    fn history(&self) -> History<'_> {
        History {
            renders: self
                .renders
                .iter()
                .map(|&(n, s, stack_name, name, picked, d, batched)| RenderEntry {
                    id: &self.ids[n as usize],
                    stack: &self.stacks[s as usize],
                    stack_name,
                    name,
                    picked,
                    created: at(n),
                    day: day(d),
                    batch: batched.then_some(&self.batch),
                })
                .collect(),
            failures: self
                .failures
                .iter()
                .map(|&(n, s, label, d)| FailureEntry {
                    id: &self.ids[n as usize],
                    stack: &self.stacks[s as usize],
                    label,
                    created: at(n),
                    day: day(d),
                    batch: &self.batch,
                })
                .collect(),
            unstacked: self
                .unstacked
                .iter()
                .map(|&(s, name, named)| UnstackedEntry {
                    stack: &self.stacks[s as usize],
                    name,
                    named,
                })
                .collect(),
        }
    }

    /// Each tile as "3:r12" (stack 3, render 12) or "4:f20" (failure 20);
    /// runs split by "|", a batch's in brackets.
    fn shown(&self, filter: Filter, query: &str) -> (Vec<String>, Vec<u64>) {
        let words = Words::english(Platform::Desktop);
        let tiles = tiles(&self.history(), filter, query, day(18), &words);
        let number = |s: &StackId| self.stacks.iter().position(|t| t == s).unwrap();
        let render = |r: &RenderId| self.ids.iter().position(|i| i == r).unwrap();
        let tile = |t: &Tile| match &t.kind {
            TileKind::Renders { cover, .. } => format!("{}:r{}", number(&t.stack), render(cover)),
            TileKind::Failed { failure } => format!("{}:f{}", number(&t.stack), render(failure)),
        };
        let days = tiles
            .days
            .iter()
            .map(|d| {
                let runs: Vec<String> = d
                    .runs
                    .iter()
                    .map(|r| match r {
                        Run::Loose(tiles) => tiles.iter().map(tile).collect::<Vec<_>>().join(" "),
                        Run::Batch { tiles, .. } => {
                            format!("[{}]", tiles.iter().map(tile).collect::<Vec<_>>().join(" "))
                        }
                    })
                    .collect();
                format!("{}: {}", d.day.format("%d"), runs.join(" | "))
            })
            .collect();
        let unstacked = tiles.unstacked.iter().map(|&i| self.unstacked[i].0).collect();
        (days, unstacked)
    }
}

/// Stack 1 has three renders (one starred), stack 2 one, stack 3 was
/// made by a batch that also failed stack 4 and stack 5, and stack 5 was
/// stacked by hand since. Stack 6 is not stacked yet.
fn library() -> Fixture {
    let mut f = Fixture::new();
    f.renders = vec![
        (1, 1, "bee", None, false, 10, false),
        (2, 1, "bee", Some("wings"), true, 16, false),
        (3, 2, "ant", None, false, 17, false),
        (5, 1, "bee", None, false, 18, false),
        (12, 3, "fly", None, false, 12, true),
        (40, 5, "moth", None, false, 18, false),
    ];
    f.failures = vec![(13, 4, "wasp", 12), (14, 5, "moth", 12)];
    f.unstacked = vec![(6, "Zebra", true)];
    f
}

#[test]
fn a_filter_and_a_search_let_tiles_through() {
    let f = library();
    let cases: [(Filter, &str, &[&str], &[u64]); 9] = [
        // Every stack once, at its newest, starred cover; stack 5's failure
        // hidden by its render; the batch under its day.
        (Filter::All, "", &["18: 5:r40 1:r2", "17: 2:r3", "12: [3:r12 4:f13]"], &[6]),
        // A starred render covers its stack, and only it shows.
        (Filter::Starred, "", &["16: 1:r2"], &[]),
        // A failed stack with no render, never one stacked since.
        (Filter::Failed, "", &["12: [4:f13]"], &[]),
        // By the stack's name, the render's own, the batch's label.
        (Filter::All, "BEE", &["18: 1:r2"], &[]),
        (Filter::All, "wings", &["16: 1:r2"], &[]),
        (Filter::All, " auto ", &["12: [3:r12 4:f13]"], &[]),
        // A failure by its label; one not stacked yet by its name.
        (Filter::Failed, "wasp", &["12: [4:f13]"], &[]),
        (Filter::All, "zeb", &[], &[6]),
        (Filter::Starred, "ant", &[], &[]),
    ];
    for (filter, query, days, unstacked) in cases {
        let (shown_days, shown_unstacked) = f.shown(filter, query);
        assert_eq!(shown_days, days, "{filter:?} {query:?}");
        assert_eq!(shown_unstacked, unstacked, "{filter:?} {query:?}");
    }
}

/// A failed batch stack opened and left shows once under All as not
/// stacked yet, and under Failed as the failure.
#[test]
fn a_failed_stack_left_with_frames_shows_once() {
    let mut f = Fixture::new();
    f.failures = vec![(13, 4, "wasp", 12)];
    f.unstacked = vec![(4, "wasp", true)];
    assert_eq!(f.shown(Filter::All, ""), (vec![], vec![4]));
    assert_eq!(f.shown(Filter::Failed, ""), (vec!["12: [4:f13]".to_string()], vec![]));
    let words = Words::english(Platform::Desktop);
    let counts = tiles(&f.history(), Filter::All, "", day(18), &words).counts;
    assert_eq!((counts.stacks, counts.failed_stacks), (1, 1));
}

#[test]
fn the_unstacked_go_by_name_the_unnamed_last() {
    let mut f = Fixture::new();
    f.unstacked =
        vec![(1, "Untitled stack", false), (2, "zebra", true), (3, "Ant", true), (4, "bee", true)];
    assert_eq!(f.shown(Filter::All, "").1, [3, 4, 2, 1]);
    assert_eq!(f.shown(Filter::Starred, "").1, Vec::<u64>::new());
}

#[test]
fn jump_to_counts_the_stacks_shown_in_each_span() {
    let f = library();
    let words = Words::english(Platform::Desktop);
    let spans = |query: &str| {
        tiles(&f.history(), Filter::All, query, day(18), &words)
            .spans
            .map(|s| (s.span, s.stacks, s.first))
    };
    assert_eq!(
        spans(""),
        [
            (Span::Today, 2, Some(0)),
            (Span::Yesterday, 1, Some(1)),
            (Span::LastWeek, 2, Some(2)),
            (Span::Earlier, 0, None),
        ]
    );
    assert_eq!(spans("ant")[1], (Span::Yesterday, 1, Some(0)));
    assert_eq!(spans("ant")[0], (Span::Today, 0, None));
}

#[test]
fn a_span_is_counted_back_from_today() {
    let today = day(18);
    let cases = [(19, Span::Today), (18, Span::Today), (17, Span::Yesterday), (16, Span::LastWeek)];
    for (d, span) in cases {
        assert_eq!(Span::of(day(d), today), span, "{d}");
    }
    assert_eq!(Span::of(day(11), today), Span::LastWeek, "seven days before yesterday");
    assert_eq!(Span::of(day(10), today), Span::Earlier);
}

#[test]
fn a_batch_run_says_when_it_started_and_how_many_failed() {
    let f = library();
    let words = Words::english(Platform::Desktop);
    let tiles = tiles(&f.history(), Filter::All, "", day(18), &words);
    let Run::Batch { started, failed, tiles: run, .. } = &tiles.days[2].runs[0] else {
        panic!("a batch run")
    };
    assert_eq!((*started, *failed, run.len()), (at(12), 1, 2));
    assert_eq!(tiles.failed_shown(), [stack(4)]);
}

#[test]
fn the_counts_and_their_lines() {
    let f = library();
    let words = Words::english(Platform::Desktop);
    let counts = tiles(&f.history(), Filter::Starred, "bee", day(18), &words).counts;
    assert_eq!(
        counts,
        Counts { stacks: 6, renders: 6, starred_renders: 1, starred_stacks: 1, failed_stacks: 1 },
        "the whole Library's, whatever is shown"
    );
    assert_eq!([Filter::All, Filter::Starred, Filter::Failed].map(|f| counts.of(f)), [6, 1, 1]);
    let line = |filter, size| counts.line(filter, size, &words);
    assert_eq!(line(Filter::All, None), "6 stacks · 6 renders");
    assert_eq!(line(Filter::All, Some("4.2 GB")), "6 stacks · 6 renders · 4.2 GB");
    assert_eq!(line(Filter::Starred, None), "1 starred render in 1 stack");
    assert_eq!(line(Filter::Failed, None), "1 failed stack");
    let none = Counts::default();
    assert_eq!(none.line(Filter::All, Some("0 KB"), &words), "No stacks · No renders");
    assert_eq!(none.line(Filter::Starred, None, &words), "No starred renders");
    assert_eq!(none.line(Filter::Failed, None, &words), "No failed stacks");
    let many = Counts { starred_renders: 5, starred_stacks: 3, ..none };
    assert_eq!(many.line(Filter::Starred, None, &words), "5 starred renders in 3 stacks");
}

#[test]
fn a_badge_counts_the_renders_let_through() {
    let words = Words::english(Platform::Desktop);
    let tile = |shown: &[u64], total| Tile {
        stack: stack(1),
        kind: TileKind::Renders {
            cover: id(shown[0]),
            shown: shown.iter().map(|&n| id(n)).collect(),
            total,
        },
    };
    assert_eq!(tile(&[1], 1).badge(&words), None);
    assert_eq!(tile(&[1, 2, 3, 4], 4).badge(&words).as_deref(), Some("4"));
    assert_eq!(tile(&[1, 2], 4).badge(&words).as_deref(), Some("2 of 4"));
    let failed = Tile { stack: stack(1), kind: TileKind::Failed { failure: id(1) } };
    assert_eq!(failed.badge(&words), None);
}
