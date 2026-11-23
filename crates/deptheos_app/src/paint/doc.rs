//! A render's paint (its doc), and every doc while the app runs, so
//! history outlives render switches, under one RAM budget.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::canvas::{Canvas, Layer};
use super::source::{SourceKind, SourceTable};
use super::store::{PaintFile, Snapshot};
use super::tile::{ORIGINAL, TileData, TileId};
use super::undo::{KEPT_STEPS, UndoStack};
use crate::library::history::Frames;
use crate::words::{Words, say, sentences};
use crate::{CompletedStack, RenderId};

/// A doc's pixels: the layer while closed, a canvas over the render while
/// open, or only the size once its file holds every tile.
///
/// ```text
/// Layer ──open──▶ Open ──close──▶ Layer ──release──▶ Released
///   ▲                                                   │
///   └──────────────────────restore──────────────────────┘
/// ```
///
/// `release` only while nothing is unsaved; a failed `restore` leaves it
/// released, and the caller reads the paint afresh.
enum Pixels {
    Layer(Layer),
    Open(Canvas),
    Released { width: u32, height: u32 },
}

impl Pixels {
    fn name(&self) -> &'static str {
        match self {
            Self::Layer(_) => "layer",
            Self::Open(_) => "open",
            Self::Released { .. } => "released",
        }
    }
}

/// Where a doc is kept: its render's directory, or memory only
/// ([`Unsaveable`]). Set as the doc is made (`store.rs`), never changed.
pub(crate) enum Storage {
    Saved(Arc<PaintFile>),
    Memory(Unsaveable),
}

/// Why a doc is kept in memory only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsaveable {
    /// Its render isn't in the Library.
    NoRender,
    /// Its saved retouching couldn't be read just now; saving over it
    /// would lose it.
    Unreachable,
    /// Its folder couldn't be written, or damaged retouching moved aside.
    Unwritable,
}

impl Unsaveable {
    /// That nothing painted can be saved, then why.
    pub fn text(self, words: &Words) -> String {
        let why = match self {
            Self::NoRender => say!(words, "paint-unsaveable-no-render"),
            Self::Unreachable => say!(words, "paint-unsaveable-unreachable"),
            Self::Unwritable => say!(words, "paint-unsaveable-unwritable"),
        };
        sentences(words, [say!(words, "paint-unsaveable"), why])
    }
}

/// One render's paint.
pub struct PaintDoc {
    pixels: Pixels,
    pub history: UndoStack,
    pub sources: SourceTable,
    pub(crate) storage: Storage,
    /// Saved while it equals [`PaintFile::committed`].
    pub(crate) changes: u64,
    /// Bumped whenever the painted pixels change.
    pub(crate) revision: u64,
}

impl PaintDoc {
    /// A doc kept in RAM only, with no render (`store` makes saved ones).
    pub fn new(layer: Layer, history: UndoStack, sources: SourceTable) -> Self {
        Self {
            pixels: Pixels::Layer(layer),
            history,
            sources,
            storage: Storage::Memory(Unsaveable::NoRender),
            changes: 0,
            revision: 0,
        }
    }

    fn set_pixels(&mut self, next: Pixels) {
        tracing::debug!(from = self.pixels.name(), to = next.name(), "paint doc pixels");
        self.pixels = next;
    }

    /// Put the layer over `base` to paint. A layer of another size is
    /// dropped with its history.
    ///
    /// # Panics
    /// On a released doc: [`Self::restore`] it first.
    pub fn open(&mut self, base: Arc<CompletedStack>) {
        let mut layer = self.take_layer();
        if [layer.width, layer.height] != [base.width, base.height] {
            tracing::warn!(
                "dropping retouching painted at {}x{} on a {}x{} render",
                layer.width,
                layer.height,
                base.width,
                base.height
            );
            layer = Layer::empty(base.width, base.height);
            self.history.clear();
            self.mark_changed();
        }
        self.set_pixels(Pixels::Open(Canvas::with_layer(base, layer)));
    }

    /// Let go of the render's pixels, keeping the layer.
    pub fn close(&mut self) {
        if let Pixels::Open(_) = self.pixels {
            let layer = self.take_layer();
            self.set_pixels(Pixels::Layer(layer));
        }
    }

    /// Close, and release the painted tiles once committed.
    pub(crate) fn put_away(&mut self) {
        self.close();
        self.release();
    }

    /// Drop a closed, fully committed doc's painted tiles, for
    /// [`Self::restore`] to read back; spilled snapshots shared with the
    /// layer leave RAM with it. Whether they went.
    pub(crate) fn release(&mut self) -> bool {
        let Pixels::Layer(layer) = &self.pixels else { return false };
        if self.persist().is_none() || self.is_unsaved() {
            return false;
        }
        if layer.painted().next().is_none() {
            return false;
        }
        let held: HashSet<*const TileData> = layer.painted().map(|(_, t)| Arc::as_ptr(t)).collect();
        let (width, height) = (layer.width, layer.height);
        self.set_pixels(Pixels::Released { width, height });
        for blob in self.history.blobs() {
            blob.evict_if_holding(&held);
        }
        true
    }

    /// Read back what [`Self::release`] dropped; on an error it stays
    /// released.
    pub(crate) fn restore(&mut self) -> std::io::Result<()> {
        let Pixels::Released { width, height } = self.pixels else { return Ok(()) };
        let file = self
            .persist()
            .ok_or_else(|| std::io::Error::other("a released doc has no file to read back from"))?;
        let layer = file.read_back(width, height)?;
        self.set_pixels(Pixels::Layer(layer));
        Ok(())
    }

    fn take_layer(&mut self) -> Layer {
        match std::mem::replace(&mut self.pixels, Pixels::Layer(Layer::empty(0, 0))) {
            Pixels::Layer(layer) => layer,
            Pixels::Open(canvas) => canvas.into_layer(),
            Pixels::Released { .. } => panic!("a released doc is restored before it opens"),
        }
    }

    #[cfg(test)]
    pub(crate) fn is_released(&self) -> bool {
        matches!(self.pixels, Pixels::Released { .. })
    }

    /// The canvas, while open.
    pub fn canvas(&self) -> Option<&Canvas> {
        match &self.pixels {
            Pixels::Open(canvas) => Some(canvas),
            Pixels::Layer(_) | Pixels::Released { .. } => None,
        }
    }

    pub fn canvas_mut(&mut self) -> Option<&mut Canvas> {
        match &mut self.pixels {
            Pixels::Open(canvas) => Some(canvas),
            Pixels::Layer(_) | Pixels::Released { .. } => None,
        }
    }

    /// Both at once, for undo and redo.
    pub(crate) fn parts_mut(&mut self) -> Option<(&mut Canvas, &mut UndoStack)> {
        match &mut self.pixels {
            Pixels::Open(canvas) => Some((canvas, &mut self.history)),
            Pixels::Layer(_) | Pixels::Released { .. } => None,
        }
    }

    /// The painted tiles, shared; `None` when released.
    pub fn layer(&self) -> Option<Layer> {
        match &self.pixels {
            Pixels::Open(canvas) => Some(canvas.layer()),
            Pixels::Layer(layer) => Some(layer.clone()),
            Pixels::Released { .. } => None,
        }
    }

    /// Whether anything is painted.
    pub fn is_painted(&self) -> bool {
        match &self.pixels {
            Pixels::Open(canvas) => canvas.painted().next().is_some(),
            Pixels::Layer(layer) => layer.tiles.iter().any(Option::is_some),
            // Only painted docs are released.
            Pixels::Released { .. } => true,
        }
    }

    /// The sources some painted pixel last came from, the original aside,
    /// in the order first picked; none for a released doc. Reads every
    /// painted pixel ([`SourceCounts`] follows strokes instead).
    pub fn painted_from(&self, frames: &Frames) -> Vec<SourceKind> {
        SourceCounts::of(self).kinds(&self.sources, frames)
    }

    pub(crate) fn mark_changed(&mut self) {
        self.changes += 1;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn persist(&self) -> Option<&Arc<PaintFile>> {
        match &self.storage {
            Storage::Saved(file) => Some(file),
            Storage::Memory(_) => None,
        }
    }

    /// The doc as a commit writes it now; `None` in memory only or
    /// released.
    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        let file = self.persist()?.clone();
        let layer = self.layer()?;
        let size = [layer.width, layer.height];
        let tiles = layer.painted().map(|(id, tile)| (id, tile.clone())).collect();
        let sources = self.sources.sources().to_vec();
        Some(Snapshot { file, changes: self.changes, size, tiles, sources })
    }

    /// Why the doc is in memory only.
    pub fn unsaveable(&self) -> Option<Unsaveable> {
        match &self.storage {
            Storage::Saved(_) => None,
            &Storage::Memory(why) => Some(why),
        }
    }

    /// Whether the doc has changes a commit would save.
    pub(crate) fn is_unsaved(&self) -> bool {
        self.persist().is_some_and(|file| file.committed() != self.changes)
    }

    /// Nothing left to commit, and nothing painted kept in RAM only.
    pub(crate) fn is_saved(&self) -> bool {
        match self.persist() {
            Some(_) => !self.is_unsaved(),
            None => !self.is_painted(),
        }
    }

    /// RAM-only and never painted: opening it afresh loses nothing.
    pub(crate) fn is_untouched_ram_only(&self) -> bool {
        self.persist().is_none() && !self.is_painted() && self.history.counts() == [0, 0]
    }
}

/// Painted pixels per source id, counted once, then followed by each
/// stroke's tiles alone ([`Self::restroke`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceCounts(Box<[u64; 256]>);

impl SourceCounts {
    /// Over every painted tile; none for a released doc.
    pub(crate) fn of(doc: &PaintDoc) -> Self {
        let mut counts = Self(Box::new([0; 256]));
        match &doc.pixels {
            Pixels::Open(canvas) => canvas.painted().for_each(|(_, t)| counts.add(t)),
            Pixels::Layer(layer) => layer.tiles.iter().flatten().for_each(|t| counts.add(t)),
            Pixels::Released { .. } => {}
        }
        counts
    }

    fn tally(tile: &TileData) -> [u64; 256] {
        let mut found = [0u64; 256];
        for (&source, &coverage) in tile.source.iter().zip(tile.coverage.iter()) {
            if coverage > 0 {
                found[usize::from(source)] += 1;
            }
        }
        found
    }

    fn add(&mut self, tile: &TileData) {
        for (count, n) in self.0.iter_mut().zip(Self::tally(tile)) {
            *count += n;
        }
    }

    fn remove(&mut self, tile: &TileData) {
        for (count, n) in self.0.iter_mut().zip(Self::tally(tile)) {
            *count = count.saturating_sub(n);
        }
    }

    /// Follow a stroke just ended: `before` is its undo entry.
    pub(crate) fn restroke(&mut self, before: &[(TileId, Option<Arc<TileData>>)], canvas: &Canvas) {
        for (id, old) in before {
            if let Some(old) = old {
                self.remove(old);
            }
            if let Some(now) = canvas.tile(*id) {
                self.add(now);
            }
        }
    }

    /// The sources counted, the original aside; a frame whose root is gone
    /// is left out.
    pub(crate) fn kinds(&self, sources: &SourceTable, frames: &Frames) -> Vec<SourceKind> {
        (0..=255u8)
            .filter(|&id| id != ORIGINAL && self.0[usize::from(id)] > 0)
            .filter_map(|id| sources.kind(id, frames))
            .collect()
    }
}

/// The app's docs, keyed by render.
#[derive(Default)]
pub(crate) struct PaintDocs {
    docs: HashMap<RenderId, PaintDoc>,
}

impl PaintDocs {
    pub(crate) fn get(&self, key: &RenderId) -> Option<&PaintDoc> {
        self.docs.get(key)
    }

    pub(crate) fn get_mut(&mut self, key: &RenderId) -> Option<&mut PaintDoc> {
        self.docs.get_mut(key)
    }

    pub(crate) fn insert(&mut self, key: RenderId, doc: PaintDoc) -> &mut PaintDoc {
        self.docs.entry(key).insert_entry(doc).into_mut()
    }

    pub(crate) fn remove(&mut self, key: &RenderId) -> Option<PaintDoc> {
        self.docs.remove(key)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&RenderId, &PaintDoc)> {
        self.docs.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&RenderId, &mut PaintDoc)> {
        self.docs.iter_mut()
    }

    /// RAM that resident history holds exclusively, across every doc.
    pub(crate) fn charged(&self) -> usize {
        self.docs.values().flat_map(|d| d.history.blobs()).map(|b| b.charge()).sum()
    }

    /// Evict spilled blobs, least recently used first, until history fits
    /// `budget`; returns the bytes still charged. Unwritten ones stay
    /// ([`Self::shed`] is for those that never will be).
    pub(crate) fn enforce_budget(&self, budget: usize) -> usize {
        let mut charged = self.charged();
        if charged <= budget {
            return charged;
        }
        let mut blobs: Vec<_> = self
            .docs
            .values()
            .flat_map(|d| d.history.blobs())
            .filter(|b| b.evictable() && b.charge() > 0)
            .collect();
        blobs.sort_by_key(|b| b.last_used());
        for blob in blobs {
            if charged <= budget {
                break;
            }
            charged -= blob.charge();
            blob.evict();
        }
        charged
    }
    /// Free `excess` bytes of unspillable history ([`UndoStack::shed`]);
    /// returns the bytes freed.
    pub(crate) fn shed(&mut self, excess: usize) -> usize {
        let mut freed = 0;
        for doc in self.docs.values_mut() {
            if freed >= excess {
                break;
            }
            freed += doc.history.shed(excess - freed, KEPT_STEPS);
        }
        freed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::brush::{Brush, Dab, Spacer};
    use crate::paint::stroke::tests::stack;
    use crate::paint::stroke::{CoveragePool, Stroke, StrokeSource};

    type Before = Vec<(TileId, Option<Arc<TileData>>)>;

    /// A stroke along the middle from source `id`, or erasing.
    fn stroke(doc: &mut PaintDoc, id: u8, x: [f32; 2], radius: f32) -> Before {
        let canvas = doc.canvas_mut().unwrap();
        let source = match id {
            ORIGINAL => StrokeSource::Original,
            id => StrokeSource::Image {
                id,
                pixels: Arc::new(vec![5000 * u16::from(id); canvas.base().len()]),
            },
        };
        let brush = Brush { radius, hardness: 1.0, opacity: 1.0, flow: 1.0, tolerance: None };
        let mut stroke = Stroke::new(canvas, brush, source, CoveragePool::default());
        let mut spacer = Spacer::new(&brush);
        let mut dabs = Vec::new();
        for x in x {
            spacer.to(Dab { x, y: 100.0, pressure: 1.0 }, &mut dabs);
        }
        for dab in dabs {
            stroke.dab(canvas, dab);
        }
        stroke.finish(canvas).before
    }

    /// Counts followed stroke by stroke match a full count.
    #[test]
    fn counts_followed_by_strokes_match_a_full_count() {
        let base = stack(600, 200, 20000);
        let mut doc = PaintDoc::new(
            Layer::empty(base.width, base.height),
            UndoStack::new(None),
            SourceTable::default(),
        );
        let (_library, frames, paths) = crate::paint::source::tests::library(3);
        let frame = |n: u8| SourceKind::Frame { path: paths[usize::from(n) - 1].clone() };
        for n in 1..=3 {
            assert_eq!(doc.sources.id(&frame(n), &frames).unwrap(), Some(n));
        }
        doc.open(base);
        let mut counts = SourceCounts::of(&doc);
        let strokes: [(u8, [f32; 2], f32, &[u8]); 6] = [
            (1, [20.0, 300.0], 40.0, &[1]),
            (2, [200.0, 580.0], 30.0, &[1, 2]),
            (3, [100.0, 140.0], 20.0, &[1, 2, 3]),
            // Frame 3's paint painted over by frame 1's.
            (1, [60.0, 180.0], 60.0, &[1, 2]),
            (ORIGINAL, [150.0, 590.0], 70.0, &[1]),
            (ORIGINAL, [0.0, 600.0], 90.0, &[]),
        ];
        for (id, x, radius, expected) in strokes {
            let before = stroke(&mut doc, id, x, radius);
            counts.restroke(&before, doc.canvas().unwrap());
            assert_eq!(counts, SourceCounts::of(&doc), "after a stroke from {id}");
            let expected: Vec<_> = expected.iter().map(|&n| frame(n)).collect();
            assert_eq!(counts.kinds(&doc.sources, &frames), expected);
            assert_eq!(doc.painted_from(&frames), expected);
        }
    }
}
