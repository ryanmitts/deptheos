//! The Library's strip: renders and failed stacks as one tile per stack,
//! placed by day and by the batch that made them.

use std::collections::HashMap;

use super::{Batch, RenderId, StackId};

/// A render or failed stack the strip shows, as `key`, made on `day`.
#[derive(Clone, Debug)]
pub struct StripEntry<K, D> {
    pub key: K,
    pub id: RenderId,
    pub day: D,
    pub batch: Option<Batch>,
}

/// A render on its own, or a batch run and what it made, oldest first.
#[derive(Clone, Debug, PartialEq)]
pub enum StripRun<K> {
    Single(K),
    Batch(Batch, Vec<K>),
}

/// A day of the strip, its runs newest first.
#[derive(Clone, Debug, PartialEq)]
pub struct StripDay<K, D> {
    pub day: D,
    pub runs: Vec<StripRun<K>>,
}

/// A stack's one tile, which [`strip`] places by day and batch.
#[derive(Clone, Debug, PartialEq)]
pub struct StackTile<K> {
    pub stack: StackId,
    /// Its entries that are shown, newest first.
    pub members: Vec<K>,
}

/// `shown` as both front ends lay the Library out: a tile per stack
/// ([`stack_tiles`]), newest day first, each batch whole under its first
/// day ([`strip_days`]). `all` includes what a search hides, so a search
/// doesn't move a batch ([`batch_days`]).
pub fn strip<'a, K, D: Ord + Copy + 'a>(
    shown: Vec<(StackId, StripEntry<K, D>)>,
    all: impl IntoIterator<Item = (&'a Batch, D)>,
) -> Vec<StripDay<StackTile<K>, D>> {
    strip_days(stack_tiles(shown), &batch_days(all))
}

/// One entry per stack, at its newest shown entry, in the batch that made
/// it even when stacked again by hand.
fn stack_tiles<K, D: Copy>(
    mut shown: Vec<(StackId, StripEntry<K, D>)>,
) -> Vec<StripEntry<StackTile<K>, D>> {
    // Ids sort by creation time: newest first.
    shown.sort_by(|a, b| b.1.id.cmp(&a.1.id));
    let mut tiles: Vec<StripEntry<StackTile<K>, D>> = Vec::new();
    let mut placed: HashMap<StackId, usize> = HashMap::new();
    for (stack, entry) in shown {
        match placed.get(&stack) {
            Some(&i) => {
                let tile = &mut tiles[i];
                tile.key.members.push(entry.key);
                // Older entries are seen later: the oldest batch wins.
                if entry.batch.is_some() {
                    tile.batch = entry.batch;
                }
            }
            None => {
                placed.insert(stack.clone(), tiles.len());
                tiles.push(StripEntry {
                    key: StackTile { stack, members: vec![entry.key] },
                    id: entry.id,
                    day: entry.day,
                    batch: entry.batch,
                });
            }
        }
    }
    tiles
}

/// The day each batch started, its oldest entry's; give it the whole
/// history so a search doesn't move a batch.
fn batch_days<'a, D: Ord + Copy>(
    entries: impl IntoIterator<Item = (&'a Batch, D)>,
) -> HashMap<String, D> {
    let mut days: HashMap<String, D> = HashMap::new();
    for (batch, day) in entries {
        days.entry(batch.id.clone()).and_modify(|d| *d = (*d).min(day)).or_insert(day);
    }
    days
}

/// `shown` by day, newest first, each day's runs newest first; a batch sits
/// whole under the day it `started` ([`batch_days`]).
fn strip_days<K, D: Ord + Copy>(
    mut shown: Vec<StripEntry<K, D>>,
    started: &HashMap<String, D>,
) -> Vec<StripDay<K, D>> {
    // Ids sort by creation time.
    shown.sort_by(|a, b| b.id.cmp(&a.id));
    let mut runs: Vec<(D, StripRun<K>)> = Vec::new();
    let mut placed: HashMap<String, usize> = HashMap::new();
    for entry in shown {
        let Some(batch) = entry.batch else {
            runs.push((entry.day, StripRun::Single(entry.key)));
            continue;
        };
        match placed.get(&batch.id) {
            Some(&i) => {
                if let StripRun::Batch(_, members) = &mut runs[i].1 {
                    members.push(entry.key);
                }
            }
            None => {
                placed.insert(batch.id.clone(), runs.len());
                let day = started.get(&batch.id).copied().unwrap_or(entry.day);
                runs.push((day, StripRun::Batch(batch, vec![entry.key])));
            }
        }
    }
    // A batch reads in the order it was made.
    for (_, run) in &mut runs {
        if let StripRun::Batch(_, members) = run {
            members.reverse();
        }
    }
    // Newest day first; within a day, newest first (the sort is stable).
    runs.sort_by_key(|(day, _)| std::cmp::Reverse(*day));
    let mut days: Vec<StripDay<K, D>> = Vec::new();
    for (day, run) in runs {
        match days.last_mut() {
            Some(last) if last.day == day => last.runs.push(run),
            _ => days.push(StripDay { day, runs: vec![run] }),
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_groups_days_and_batches() {
        let id = |n: u64| RenderId::parse(&format!("{n:032x}")).unwrap();
        let batch = Batch {
            id: "b".into(),
            label: crate::library::history::BatchLabel {
                split: crate::runs::batch::Split::AutoSort,
                skipped: 0,
            },
        };
        let entry = |n: u64, day: u32, batch: Option<&Batch>| StripEntry {
            key: n,
            id: id(n),
            day,
            batch: batch.cloned(),
        };
        // The batch ran from day 1 into day 2; render 5 is on its own, day 2.
        let all = [entry(1, 1, Some(&batch)), entry(2, 2, Some(&batch)), entry(3, 2, Some(&batch))];
        let started = batch_days(all.iter().map(|e| (e.batch.as_ref().unwrap(), e.day)));
        let mut shown = all.to_vec();
        shown.push(entry(5, 2, None));
        shown.push(entry(4, 1, None));
        let days = strip_days(shown, &started);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0], StripDay { day: 2, runs: vec![StripRun::Single(5)] });
        assert_eq!(
            days[1],
            StripDay {
                day: 1,
                runs: vec![StripRun::Single(4), StripRun::Batch(batch, vec![1, 2, 3])]
            }
        );
    }

    #[test]
    fn stacks_are_one_tile_each() {
        let id = |n: u64| RenderId::parse(&format!("{n:032x}")).unwrap();
        let stack = |n: u64| StackId::parse(&id(n).0).unwrap();
        let batch = Batch {
            id: "b".into(),
            label: crate::library::history::BatchLabel {
                split: crate::runs::batch::Split::AutoSort,
                skipped: 0,
            },
        };
        let entry = |n: u64, day: u32, batch: Option<&Batch>| StripEntry {
            key: n,
            id: id(n),
            day,
            batch: batch.cloned(),
        };
        // Stack 100: batch on day 1 (render 1), by hand on day 3 (render 4).
        // Stack 200: renders 2 and 3.
        let shown = vec![
            (stack(100), entry(1, 1, Some(&batch))),
            (stack(200), entry(2, 2, None)),
            (stack(200), entry(3, 2, None)),
            (stack(100), entry(4, 3, None)),
        ];
        let tiles = stack_tiles(shown);
        assert_eq!(tiles.len(), 2);
        // Newest stack first, each at its newest render, members newest first.
        assert_eq!(tiles[0].key, StackTile { stack: stack(100), members: vec![4, 1] });
        assert_eq!((tiles[0].id.clone(), tiles[0].day), (id(4), 3));
        assert_eq!(tiles[0].batch, Some(batch.clone()), "it stays in the batch that made it");
        assert_eq!(tiles[1].key, StackTile { stack: stack(200), members: vec![3, 2] });
        assert_eq!(tiles[1].batch, None);

        // The batch stack sits under its batch's first day.
        let started = batch_days([(&batch, 1)]);
        let days = strip_days(tiles, &started);
        let days: Vec<u32> = days.iter().map(|d| d.day).collect();
        assert_eq!(days, [2, 1]);
    }
}
