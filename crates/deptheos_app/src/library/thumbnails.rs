//! Which render thumbnails to read, keep and let go of, for both apps: a
//! pure reducer on the UI thread (ADR-144), [`Event`]s in and [`Action`]s
//! out. The pixels stay the app's, changed only by [`Action::Keep`] and
//! [`Action::Drop`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::library::history::RenderId;

/// How long before a failed thumbnail is read again when wanted, up to
/// [`TRIES`] reads: a sleeping drive gets another go, a missing file isn't
/// read each time it shows.
pub const RETRY_AFTER: Duration = Duration::from_secs(5);
/// How many reads a thumbnail gets before it is left as failed.
pub const TRIES: u32 = 3;
/// The most thumbnails one read asks for, bounding the wait of what comes
/// into view meanwhile.
pub const PER_READ: usize = 24;

/// One render's thumbnail; a render with none is not in [`Thumbnails`].
///
/// Lifecycle: none, reading, shown, failed; changed only by
/// [`Thumbnails::on`] (ADR-144), a failed one read at most [`TRIES`]
/// times. The pixels are each app's: `deptheos_gpui/src/renders/thumbnails.rs`
/// and iOS's `HistoryModel.thumbnails` (`Model/HistoryModel.swift`).
///
/// ```text
/// (none) ──wanted──▶ Reading ──read──▶ Shown ──dropped past the most kept, or shed──▶ (none)
///                       └──couldn't be──▶ Failed ──wanted, RETRY_AFTER on──▶ Reading
/// (none), Reading, Failed ──its render saved──▶ Shown
/// any ──its render deleted, or the Library──▶ (none)
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thumb {
    /// In the read in flight; `tries` reads of it failed before.
    Reading { tries: u32 },
    /// The app holds its pixels.
    Shown,
    /// `tries` reads of it failed, the last at `last`.
    Failed { tries: u32, last: Instant },
}

impl Thumb {
    fn name(&self) -> &'static str {
        match self {
            Self::Reading { .. } => "reading",
            Self::Shown => "shown",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Which read an [`Action::Read`] started; its results count only while it
/// is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReadToken(pub u64);

/// What happened.
#[derive(Clone, Debug)]
pub enum Event {
    /// These are wanted now, most wanted first, replacing what was; none is
    /// dropped to make room.
    Wanted { ids: Vec<RenderId>, now: Instant },
    /// One thumbnail of read `token` was read, or could not be (`ok`).
    Read { token: ReadToken, id: RenderId, ok: bool, now: Instant },
    /// Read `token` is over. Those wanted meanwhile are read next, unless it
    /// left some unreported.
    Finished { token: ReadToken, now: Instant },
    /// A render was saved and the app has its thumbnail from the save.
    Saved { id: RenderId },
    /// These renders were deleted: nothing of them is wanted or kept.
    Deleted { ids: Vec<RenderId> },
    /// Memory is short: keep only what is wanted now.
    Shed,
    /// The whole Library was deleted.
    Cleared,
}

/// What the app is to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Read `ids` in turn, reporting each ([`Event::Read`]) and the end
    /// ([`Event::Finished`]).
    Read { token: ReadToken, ids: Vec<RenderId> },
    /// Keep the pixels just read or saved for `id`; others read are let go.
    Keep(RenderId),
    /// Let go of `id`'s pixels.
    Drop(RenderId),
}

/// A render's thumbnail and when it was last wanted.
#[derive(Clone, Debug)]
struct Entry {
    thumb: Thumb,
    /// A count of [`Event::Wanted`]; the oldest are dropped first.
    used: u64,
}

/// Every render's [`Thumb`], what is wanted and the read in flight.
#[derive(Clone, Debug)]
pub struct Thumbnails {
    entries: HashMap<RenderId, Entry>,
    /// What was wanted last, most wanted first.
    wanted: Vec<RenderId>,
    /// How many [`Event::Wanted`] there have been.
    tick: u64,
    /// The most thumbnails shown at once.
    kept: usize,
    /// The read in flight: one at a time.
    reading: Option<ReadToken>,
    /// The last token handed out.
    last_token: u64,
}

impl Thumbnails {
    /// No thumbnail yet; at most `kept` shown at once (the app's figure).
    pub fn new(kept: usize) -> Self {
        Self {
            entries: HashMap::new(),
            wanted: Vec::new(),
            tick: 0,
            kept,
            reading: None,
            last_token: 0,
        }
    }

    /// `id`'s thumbnail, if it has one.
    pub fn get(&self, id: &RenderId) -> Option<Thumb> {
        self.entries.get(id).map(|e| e.thumb)
    }

    /// Every render with a thumbnail, and what it is.
    pub fn thumbs(&self) -> impl Iterator<Item = (&RenderId, Thumb)> {
        self.entries.iter().map(|(id, e)| (id, e.thumb))
    }

    /// Show at most `kept` from the next event on.
    pub fn set_kept(&mut self, kept: usize) {
        self.kept = kept;
    }

    /// The thumbnails shown.
    pub fn shown(&self) -> HashSet<&RenderId> {
        self.entries.iter().filter(|(_, e)| e.thumb == Thumb::Shown).map(|(id, _)| id).collect()
    }

    /// Take in `event`, and say what the app is to do. Doesn't block.
    pub fn on(&mut self, event: Event) -> Vec<Action> {
        let mut actions = Vec::new();
        match event {
            Event::Wanted { ids, now } => {
                self.tick += 1;
                for id in &ids {
                    if let Some(entry) = self.entries.get_mut(id) {
                        entry.used = self.tick;
                    }
                }
                self.wanted = ids;
                self.make_room(0, &mut actions);
                self.read_next(now, &mut actions);
            }
            Event::Read { token, id, ok, now } => {
                let reading = self.reading == Some(token);
                match self.get(&id) {
                    Some(Thumb::Reading { tries }) if reading => match ok {
                        true => {
                            self.make_room(1, &mut actions);
                            self.set(&id, Thumb::Shown);
                            actions.push(Action::Keep(id));
                        }
                        false => self.set(&id, Thumb::Failed { tries: tries + 1, last: now }),
                    },
                    // Deleted or saved meanwhile, or a stale read: not kept.
                    _ => {}
                }
            }
            Event::Finished { token, now } => {
                if self.reading == Some(token) {
                    self.reading = None;
                    // Unreported ones wait for the next want: a read that
                    // can't start would otherwise restart forever.
                    let unread: Vec<RenderId> = self
                        .entries
                        .iter()
                        .filter(|(_, e)| matches!(e.thumb, Thumb::Reading { .. }))
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in &unread {
                        self.forget(id);
                    }
                    if unread.is_empty() {
                        self.read_next(now, &mut actions);
                    }
                }
            }
            Event::Saved { id } => {
                if self.get(&id) != Some(Thumb::Shown) {
                    self.make_room(1, &mut actions);
                }
                self.set(&id, Thumb::Shown);
                actions.push(Action::Keep(id));
            }
            Event::Deleted { ids } => {
                self.wanted.retain(|w| !ids.contains(w));
                for id in ids {
                    if self.forget(&id) == Some(Thumb::Shown) {
                        actions.push(Action::Drop(id));
                    }
                }
            }
            Event::Shed => {
                let wanted: HashSet<&RenderId> = self.wanted.iter().collect();
                let off: Vec<RenderId> = self
                    .entries
                    .iter()
                    .filter(|(id, e)| e.thumb == Thumb::Shown && !wanted.contains(id))
                    .map(|(id, _)| id.clone())
                    .collect();
                tracing::debug!("thumbnails: shed {} out of view", off.len());
                for id in off {
                    self.forget(&id);
                    actions.push(Action::Drop(id));
                }
            }
            Event::Cleared => {
                for (id, entry) in std::mem::take(&mut self.entries) {
                    if entry.thumb == Thumb::Shown {
                        actions.push(Action::Drop(id));
                    }
                }
                self.wanted.clear();
                self.reading = None;
            }
        }
        actions
    }

    /// With no read in flight, read those wanted that aren't shown, read or
    /// waiting to retry.
    fn read_next(&mut self, now: Instant, actions: &mut Vec<Action>) {
        if self.reading.is_some() {
            return;
        }
        let ids: Vec<RenderId> = self
            .wanted
            .iter()
            .filter(|id| match self.get(id) {
                None => true,
                Some(Thumb::Reading { .. } | Thumb::Shown) => false,
                Some(Thumb::Failed { tries, last }) => {
                    tries < TRIES && now.duration_since(last) >= RETRY_AFTER
                }
            })
            .take(PER_READ)
            .cloned()
            .collect();
        if ids.is_empty() {
            return;
        }
        self.last_token += 1;
        let token = ReadToken(self.last_token);
        self.reading = Some(token);
        for id in &ids {
            let tries = match self.get(id) {
                Some(Thumb::Failed { tries, .. }) => tries,
                _ => 0,
            };
            self.set(id, Thumb::Reading { tries });
        }
        tracing::debug!("thumbnails: read {} ({token:?})", ids.len());
        actions.push(Action::Read { token, ids });
    }

    /// Drop the oldest wanted while more than `kept` would show with
    /// `incoming`, never one wanted now.
    fn make_room(&mut self, incoming: usize, actions: &mut Vec<Action>) {
        let shown = self.entries.values().filter(|e| e.thumb == Thumb::Shown).count();
        let Some(over) = (shown + incoming).checked_sub(self.kept) else { return };
        let wanted: HashSet<&RenderId> = self.wanted.iter().collect();
        let mut old: Vec<(u64, RenderId)> = self
            .entries
            .iter()
            .filter(|(id, e)| e.thumb == Thumb::Shown && !wanted.contains(id))
            .map(|(id, e)| (e.used, id.clone()))
            .collect();
        old.sort_unstable();
        for (_, id) in old.into_iter().take(over) {
            self.forget(&id);
            actions.push(Action::Drop(id));
        }
    }

    fn set(&mut self, id: &RenderId, thumb: Thumb) {
        let was = self.get(id).map_or("none", |t| t.name());
        tracing::debug!("thumbnail {}: {was} -> {}", id.as_str(), thumb.name());
        let used = self.tick;
        self.entries
            .entry(id.clone())
            .and_modify(|e| e.thumb = thumb)
            .or_insert(Entry { thumb, used });
    }

    /// `id` has no thumbnail now; what it had.
    fn forget(&mut self, id: &RenderId) -> Option<Thumb> {
        let entry = self.entries.remove(id)?;
        tracing::debug!("thumbnail {}: {} -> none", id.as_str(), entry.thumb.name());
        Some(entry.thumb)
    }
}

#[cfg(test)]
mod tests;
