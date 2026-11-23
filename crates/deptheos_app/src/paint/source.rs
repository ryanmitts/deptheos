//! The images strokes take their pixels from, and the few the paint mode
//! keeps ready: the active one, the previous one (for swapping back), and
//! at most one neighbor fetched ahead.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::tile::ORIGINAL;
use crate::engine::memory::Budget;
use crate::frames::sources::FrameRef;
use crate::fsutil::Result as StoreResult;
use crate::library::history::Frames;
use crate::{Moves, PresentedImage};

/// What a source id stands for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    Original,
    Frame { path: PathBuf },
    Render { render: String },
}

impl SourceKind {
    /// Point a frame under a moved folder ([`Moves`]) where it is now.
    pub fn follow(&mut self, moves: &Moves) -> bool {
        match self {
            Self::Frame { path } => match moves.path(path) {
                Some(to) => {
                    *path = to;
                    true
                }
                None => false,
            },
            Self::Original | Self::Render { .. } => false,
        }
    }
}

/// A source as the doc keeps it: a frame by its root (ADR-145), so a
/// relink changes nothing here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Original,
    Frame(FrameRef),
    Render(String),
}

/// The doc's source ids. Id 0 is always the original.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceTable(Vec<Source>);

impl Default for SourceTable {
    fn default() -> Self {
        Self(vec![Source::Original])
    }
}

impl SourceTable {
    /// The id of `kind`, assigning the next when new; `None` once all 256
    /// are taken. A frame is named by root through `frames`, which may add
    /// one.
    pub fn id(&mut self, kind: &SourceKind, frames: &Frames) -> StoreResult<Option<u8>> {
        let source = match kind {
            SourceKind::Original => Source::Original,
            SourceKind::Frame { path } => {
                Source::Frame(frames.refs(std::slice::from_ref(path))?.remove(0))
            }
            SourceKind::Render { render } => Source::Render(render.clone()),
        };
        let roots = frames.sources();
        let same = |a: &Source| match (a, &source) {
            (Source::Frame(a), Source::Frame(b)) => roots.identity(a).same_file(&roots.identity(b)),
            (a, b) => a == b,
        };
        if let Some(i) = self.0.iter().position(same) {
            return Ok(Some(i as u8));
        }
        let Ok(id) = u8::try_from(self.0.len()) else { return Ok(None) };
        self.0.push(source);
        Ok(Some(id))
    }

    /// `None` for an unknown id or a frame whose root is gone.
    pub fn kind(&self, id: u8, frames: &Frames) -> Option<SourceKind> {
        Some(match self.0.get(usize::from(id))? {
            Source::Original => SourceKind::Original,
            Source::Frame(frame) => SourceKind::Frame {
                path: frames.paths(std::slice::from_ref(frame)).ok()?.remove(0),
            },
            Source::Render(render) => SourceKind::Render { render: render.clone() },
        })
    }

    /// In id order.
    pub(crate) fn sources(&self) -> &[Source] {
        &self.0
    }

    /// A table read back; check it with [`Self::is_valid`].
    pub(crate) fn of(sources: Vec<Source>) -> Self {
        Self(sources)
    }

    /// Whether [`Self::id`] could have built it: the original first, at
    /// most 256 ids, none twice.
    pub(crate) fn is_valid(&self) -> bool {
        self.0.first() == Some(&Source::Original)
            && self.0.len() <= 256
            && self.0.iter().enumerate().all(|(i, k)| !self.0[..i].contains(k))
    }
}

const _: () = assert!(ORIGINAL == 0, "the source table puts the original at 0");

/// A source ready to paint from and to show.
pub(crate) struct PaintSource {
    pub kind: SourceKind,
    /// In the canvas's space. `None` for the original (strokes read the
    /// canvas's base) and for a source built only for show.
    pub pixels: Option<Arc<Vec<u16>>>,
    pub display: Arc<PresentedImage>,
}

impl PaintSource {
    /// Whether strokes can paint from it now.
    pub(crate) fn is_paintable(&self) -> bool {
        self.pixels.is_some() || self.kind == SourceKind::Original
    }

    pub(crate) fn bytes(&self) -> usize {
        self.pixels.as_ref().map_or(0, |p| 2 * p.len()) + self.display.bytes()
    }
}

/// The sources kept in RAM, within a budget.
pub(crate) struct SourceCache {
    /// Read live ([`Budget::PaintSources`]).
    budget: Budget,
    active: Option<Arc<PaintSource>>,
    previous: Option<Arc<PaintSource>>,
    prefetched: Option<Arc<PaintSource>>,
}

impl SourceCache {
    pub(crate) fn new(budget: Budget) -> Self {
        Self { budget, active: None, previous: None, prefetched: None }
    }

    pub(crate) fn active(&self) -> Option<&Arc<PaintSource>> {
        self.active.as_ref()
    }

    fn kept(&self) -> impl Iterator<Item = &Arc<PaintSource>> {
        [&self.active, &self.previous, &self.prefetched].into_iter().flatten()
    }

    /// A kept source of `kind`, paintable if `paintable`.
    pub(crate) fn find(&self, kind: &SourceKind, paintable: bool) -> Option<Arc<PaintSource>> {
        self.kept().find(|s| s.kind == *kind && (!paintable || s.is_paintable())).cloned()
    }

    /// Make `source` active, the old one previous; past the budget the
    /// prefetch goes first, then the previous.
    pub(crate) fn activate(&mut self, source: Arc<PaintSource>) {
        if self.prefetched.as_ref().is_some_and(|p| p.kind == source.kind) {
            self.prefetched = None;
        }
        match self.active.take() {
            Some(old) if old.kind == source.kind => {}
            Some(old) => {
                self.previous = Some(old);
            }
            None => {}
        }
        if self.previous.as_ref().is_some_and(|p| p.kind == source.kind) {
            self.previous = None;
        }
        self.active = Some(source);
        self.make_room(0);
    }

    /// Drop what `bytes` more would push past the budget, prefetch first.
    /// Called before a new source is built, so old and new aren't all in
    /// RAM at once.
    pub(crate) fn make_room(&mut self, bytes: usize) {
        let budget = self.budget.bytes(self.bytes());
        if self.bytes() + bytes > budget {
            self.prefetched = None;
        }
        if self.bytes() + bytes > budget {
            self.previous = None;
        }
    }

    /// Memory is short: drop the previous and prefetched sources; the
    /// bytes freed.
    pub(crate) fn shed(&mut self) -> usize {
        [self.previous.take(), self.prefetched.take()]
            .into_iter()
            .flatten()
            .map(|s| s.bytes())
            .sum()
    }

    /// Whether a prefetch of `bytes` would fit beside what is kept.
    pub(crate) fn fits(&self, bytes: usize) -> bool {
        let kept = self.bytes() - self.prefetched.as_ref().map_or(0, |p| p.bytes());
        kept + bytes <= self.budget.bytes(self.bytes())
    }

    /// Keep `source` as the neighbor fetched ahead, if it fits.
    pub(crate) fn prefetch(&mut self, source: Arc<PaintSource>) -> bool {
        if !self.fits(source.bytes()) {
            return false;
        }
        self.prefetched = Some(source);
        true
    }

    #[cfg(test)]
    pub fn prefetched(&self) -> Option<&Arc<PaintSource>> {
        self.prefetched.as_ref()
    }

    /// Drop everything (leaving paint mode).
    pub(crate) fn clear(&mut self) {
        *self = Self::new(self.budget);
    }

    /// Follow folders that moved ([`SourceKind::follow`]).
    pub(crate) fn follow(&mut self, moves: &Moves) {
        for source in
            [&mut self.active, &mut self.previous, &mut self.prefetched].into_iter().flatten()
        {
            let mut kind = source.kind.clone();
            if kind.follow(moves) {
                *source = Arc::new(PaintSource {
                    kind,
                    pixels: source.pixels.clone(),
                    display: source.display.clone(),
                });
            }
        }
    }

    fn bytes(&self) -> usize {
        self.kept().map(|s| s.bytes()).sum()
    }
}

/// The frame `step` places from `current` in `frames`, if any.
pub fn step_frame<'a>(frames: &'a [PathBuf], current: &Path, step: isize) -> Option<&'a PathBuf> {
    let i = frames.iter().position(|f| f == current)?;
    frames.get(i.checked_add_signed(step)?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::CpuImage;

    /// A Library with `n` frames in a folder beside it.
    pub(crate) fn library(n: usize) -> (tempfile::TempDir, Arc<Frames>, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Photos");
        std::fs::create_dir_all(&folder).unwrap();
        let paths: Vec<PathBuf> = (0..n).map(|i| folder.join(format!("frame-{i}.tif"))).collect();
        for (i, path) in paths.iter().enumerate() {
            std::fs::write(path, [i as u8]).unwrap();
        }
        let places =
            crate::frames::sources::Places { library: dir.path().join("Library"), app: None };
        std::fs::create_dir_all(&places.library).unwrap();
        (dir, Arc::new(Frames::open(places).unwrap()), paths)
    }

    /// A frame has one id however its path is written, and a relink
    /// changes nothing in the table.
    #[test]
    fn frames_are_kept_by_root() {
        let (dir, frames, paths) = library(2);
        let mut table = SourceTable::default();
        let kind = |path: &Path| SourceKind::Frame { path: path.to_path_buf() };
        assert_eq!(table.id(&kind(&paths[0]), &frames).unwrap(), Some(1));
        assert_eq!(table.id(&kind(&paths[1]), &frames).unwrap(), Some(2));
        assert_eq!(table.id(&kind(&paths[0]), &frames).unwrap(), Some(1));
        assert_eq!(table.id(&SourceKind::Original, &frames).unwrap(), Some(0));
        let before = table.clone();
        let moved = dir.path().join("Moved");
        std::fs::rename(dir.path().join("Photos"), &moved).unwrap();
        frames.relink(&dir.path().join("Photos"), &moved, None).unwrap();
        assert_eq!(table, before, "nothing kept changed");
        assert_eq!(table.kind(1, &frames), Some(kind(&moved.join("frame-0.tif"))));
        assert_eq!(table.id(&kind(&moved.join("frame-1.tif")), &frames).unwrap(), Some(2));
        assert!(table.is_valid());
    }

    fn source(name: &str, bytes: usize) -> Arc<PaintSource> {
        let image = CpuImage { width: 1, height: 1, pixels: vec![0; 4] };
        Arc::new(PaintSource {
            kind: SourceKind::Frame { path: name.into() },
            pixels: Some(Arc::new(vec![0; bytes / 2])),
            display: Arc::new(PresentedImage::single(image)),
        })
    }

    fn kind(name: &str) -> SourceKind {
        SourceKind::Frame { path: name.into() }
    }

    #[test]
    fn the_cache_keeps_the_active_and_previous_sources() {
        let mut cache = SourceCache::new(Budget::Fixed(10_000));
        cache.activate(source("a", 1000));
        cache.activate(source("b", 1000));
        assert_eq!(cache.active().unwrap().kind, kind("b"));
        assert!(cache.find(&kind("a"), true).is_some(), "the previous, to go back to at once");
        cache.activate(source("a", 1000));
        assert_eq!(cache.active().unwrap().kind, kind("a"));
        cache.activate(source("c", 1000));
        assert!(cache.find(&kind("b"), true).is_none(), "only two are kept");
        assert!(cache.find(&kind("a"), true).is_some());
    }

    #[test]
    fn the_budget_drops_the_prefetch_first_then_the_previous() {
        let mut cache = SourceCache::new(Budget::Fixed(3000));
        cache.activate(source("a", 1000));
        cache.activate(source("b", 1000));
        assert!(!cache.prefetch(source("x", 1500)), "past the budget");
        assert!(cache.prefetch(source("c", 800)));
        // Activating the prefetched one takes it out of the prefetch slot.
        cache.activate(cache.find(&kind("c"), true).unwrap());
        assert!(cache.prefetched().is_none());
        assert_eq!(cache.previous.as_ref().unwrap().kind, kind("b"));
        // One too large to keep beside the active one drops the previous.
        cache.activate(source("big", 2500));
        assert!(cache.previous.is_none());
    }

    #[test]
    fn room_is_made_before_a_new_source_is_built() {
        let mut cache = SourceCache::new(Budget::Fixed(4000));
        cache.activate(source("a", 1000));
        cache.activate(source("b", 1000));
        assert!(cache.prefetch(source("c", 800)));
        // 1500 more fits beside the active and previous, not the prefetch.
        cache.make_room(1500);
        assert!(cache.prefetched().is_none());
        assert_eq!(cache.previous.as_ref().unwrap().kind, kind("a"));
        // 2500 fits only beside the active one.
        cache.make_room(2500);
        assert!(cache.previous.is_none());
        assert_eq!(cache.active().unwrap().kind, kind("b"), "the active one stays");
    }

    #[test]
    fn a_shed_keeps_only_the_source_painted_from() {
        let mut cache = SourceCache::new(Budget::Fixed(10_000));
        cache.activate(source("a", 1000));
        cache.activate(source("b", 1000));
        assert!(cache.prefetch(source("c", 800)));
        assert_eq!(cache.shed(), 1000 + 800 + 2 * 36, "with their 3×3 displays");
        assert!(cache.previous.is_none() && cache.prefetched().is_none());
        assert_eq!(cache.active().unwrap().kind, kind("b"));
    }

    #[test]
    fn stepping_stays_in_the_frames() {
        let frames: Vec<PathBuf> = ["a", "b", "c"].map(PathBuf::from).to_vec();
        assert_eq!(step_frame(&frames, Path::new("b"), 1).unwrap(), Path::new("c"));
        assert_eq!(step_frame(&frames, Path::new("a"), -1), None);
        assert_eq!(step_frame(&frames, Path::new("c"), 1), None);
        assert_eq!(step_frame(&frames, Path::new("z"), 1), None);
    }
}
