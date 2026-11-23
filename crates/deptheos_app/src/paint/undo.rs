//! Undo and redo: entries of tile snapshots that swap with the canvas, so
//! neither copies pixels. Snapshots stay in RAM within a budget and are
//! written to the doc's spill (`store::Spill`), read back once evicted.
//! The history doesn't survive a restart.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use super::canvas::Canvas;
use super::log;
use super::store::Spill;
use super::tile::{TileData, TileId};
use crate::lock;

/// Stamps blob use, for least-recently-used eviction across docs.
static CLOCK: AtomicU64 = AtomicU64::new(0);

fn tick() -> u64 {
    CLOCK.fetch_add(1, Ordering::Relaxed)
}

/// The newest undo steps [`UndoStack::shed`] never drops. One on iOS: a
/// stroke over a whole 45 MP render holds 360 MB, so three would pass a
/// gigabyte.
pub(crate) const KEPT_STEPS: usize = if cfg!(target_os = "ios") { 1 } else { 3 };

/// Where one tile's pixels are: in RAM in every state but `Evicted`, in the
/// spill in `Stored` and `Evicted`.
///
/// Lifecycle: unwritten, stuck, stored, evicted; changed only by its
/// [`Blob`]'s methods (`write_all`, `evict`, `load`).
///
/// ```text
/// Unwritten ──write──▶ Stored ──evict──▶ Evicted ──load──▶ Stored
///     └──write fails──▶ Stuck
/// ```
///
/// A history holds thousands of blobs, so transitions log at trace.
enum BlobState {
    /// In RAM, waiting for the store thread to write it.
    Unwritten(Arc<TileData>),
    /// Its write failed: it never leaves RAM, but [`UndoStack::shed`] may
    /// drop its entry.
    Stuck(Arc<TileData>),
    /// In the spill and in RAM.
    Stored { row: i64, len: u32, tile: Arc<TileData> },
    /// In the spill alone.
    Evicted { row: i64, len: u32 },
}

impl BlobState {
    fn name(&self) -> &'static str {
        match self {
            Self::Unwritten(_) => "unwritten",
            Self::Stuck(_) => "stuck",
            Self::Stored { .. } => "stored",
            Self::Evicted { .. } => "evicted",
        }
    }

    fn tile(&self) -> Option<&Arc<TileData>> {
        match self {
            Self::Unwritten(tile) | Self::Stuck(tile) | Self::Stored { tile, .. } => Some(tile),
            Self::Evicted { .. } => None,
        }
    }

    /// Its spill row and byte length, once written.
    fn row(&self) -> Option<(i64, u32)> {
        match *self {
            Self::Stored { row, len, .. } | Self::Evicted { row, len } => Some((row, len)),
            Self::Unwritten(_) | Self::Stuck(_) => None,
        }
    }

    fn set(&mut self, next: Self) {
        tracing::trace!(from = self.name(), to = next.name(), "blob");
        *self = next;
    }
}

/// One tile's pixels in the history: in RAM, in the spill, or both.
pub struct Blob {
    state: Mutex<BlobState>,
    used: AtomicU64,
    /// `None` for a history kept in RAM only.
    spill: Option<Arc<Spill>>,
}

impl Blob {
    fn resident(tile: Arc<TileData>, spill: Option<Arc<Spill>>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(BlobState::Unwritten(tile)),
            used: AtomicU64::new(tick()),
            spill,
        })
    }

    /// Write the unwritten of `blobs` to `spill` in one transaction, on the
    /// store thread, no blob locked meanwhile (a load or eviction can't
    /// touch an unwritten blob). On failure every one is `Stuck`.
    pub(crate) fn write_all(spill: &Spill, blobs: &[Arc<Blob>]) -> io::Result<()> {
        let unwritten: Vec<(&Arc<Blob>, Arc<TileData>)> = blobs
            .iter()
            .filter_map(|blob| match &*lock(&blob.state) {
                BlobState::Unwritten(tile) => Some((blob, tile.clone())),
                _ => None,
            })
            .collect();
        if unwritten.is_empty() {
            return Ok(());
        }
        // In parallel: a stroke can be thousands of tiles.
        let encoded: Vec<Vec<u8>> =
            unwritten.par_iter().map(|(_, tile)| log::encode(tile)).collect();
        let written = spill.write(&encoded);
        for (i, (blob, tile)) in unwritten.into_iter().enumerate() {
            let mut state = lock(&blob.state);
            match &written {
                Ok(rows) => {
                    let len = u32::try_from(encoded[i].len()).unwrap_or(u32::MAX);
                    state.set(BlobState::Stored { row: rows[i], len, tile });
                }
                Err(_) => state.set(BlobState::Stuck(tile)),
            }
        }
        written.map(drop).map_err(io::Error::other)
    }

    fn stuck(&self) -> bool {
        matches!(*lock(&self.state), BlobState::Stuck(_))
    }

    fn stored_len(&self) -> Option<u32> {
        lock(&self.state).row().map(|(_, len)| len)
    }

    /// The tile, read back from the spill if evicted.
    fn load(&self) -> io::Result<Arc<TileData>> {
        self.used.store(tick(), Ordering::Relaxed);
        let row = match &*lock(&self.state) {
            &BlobState::Evicted { row, .. } => row,
            state => return Ok(state.tile().expect("in RAM unless evicted").clone()),
        };
        let spill = self.spill.as_ref().expect("only a written blob is evicted");
        let tile = Arc::new(log::decode(&spill.read(row)?)?);
        let mut state = lock(&self.state);
        if let BlobState::Evicted { row, len } = *state {
            state.set(BlobState::Stored { row, len, tile: tile.clone() });
        }
        Ok(tile)
    }

    /// The RAM this blob alone keeps alive: its tile, unless shared.
    pub(crate) fn charge(&self) -> usize {
        match lock(&self.state).tile() {
            Some(tile) if Arc::strong_count(tile) == 1 => TileData::BYTES,
            _ => 0,
        }
    }

    /// Whether dropping the RAM copy loses nothing.
    pub(crate) fn evictable(&self) -> bool {
        matches!(*lock(&self.state), BlobState::Stored { .. })
    }

    pub fn evict(&self) {
        let mut state = lock(&self.state);
        if let BlobState::Stored { row, len, .. } = *state {
            state.set(BlobState::Evicted { row, len });
        }
    }

    /// [`Self::evict`], when its tile is one of `tiles`.
    pub(crate) fn evict_if_holding(&self, tiles: &std::collections::HashSet<*const TileData>) {
        let mut state = lock(&self.state);
        if let BlobState::Stored { row, len, tile } = &*state
            && tiles.contains(&Arc::as_ptr(tile))
        {
            let (row, len) = (*row, *len);
            state.set(BlobState::Evicted { row, len });
        }
    }

    pub(crate) fn last_used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }
}

impl Drop for Blob {
    /// Its row is the spill's to delete.
    fn drop(&mut self) {
        let row = self.state.get_mut().unwrap_or_else(|p| p.into_inner()).row();
        if let (Some((row, _)), Some(spill)) = (row, &self.spill) {
            spill.free(row);
        }
    }
}

/// A tile's state at one end of an entry.
#[derive(Clone)]
pub(crate) enum Snapshot {
    Original,
    Blob(Arc<Blob>),
}

impl Snapshot {
    fn blob(&self) -> Option<&Arc<Blob>> {
        match self {
            Self::Blob(blob) => Some(blob),
            Self::Original => None,
        }
    }
}

/// One undoable action: a stroke, or a reset.
pub(crate) struct Entry {
    pub label: String,
    /// What popping the entry swaps into the canvas.
    pub tiles: Vec<(TileId, Snapshot)>,
    /// For an entry an undo or redo made: the snapshots that swap popped,
    /// which the canvas holds while it's on top, in `tiles` order. Its
    /// reverse reuses them rather than snapshotting every tile again.
    /// `None` for a stroke's entry.
    applied: Option<Vec<Snapshot>>,
}

impl Entry {
    fn blobs(&self) -> impl Iterator<Item = &Arc<Blob>> {
        let applied = self.applied.iter().flatten();
        self.tiles.iter().map(|(_, s)| s).chain(applied).filter_map(Snapshot::blob)
    }

    fn stored_bytes(&self) -> u64 {
        self.blobs().filter_map(|b| b.stored_len()).map(u64::from).sum()
    }
}

/// The tiles an undo or redo changed, and the blobs it made, which the
/// spill still needs.
pub struct Change {
    pub tiles: Vec<TileId>,
    pub new_blobs: Vec<Arc<Blob>>,
}

/// One render's undo and redo stacks.
#[derive(Default)]
pub struct UndoStack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// `None`: kept in RAM only.
    spill: Option<Arc<Spill>>,
}

impl UndoStack {
    pub(crate) fn new(spill: Option<Arc<Spill>>) -> Self {
        Self { spill, ..Self::default() }
    }

    pub(crate) fn spill(&self) -> Option<&Arc<Spill>> {
        self.spill.as_ref()
    }

    /// Drop every entry; their rows are freed.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// The undo and redo entry counts.
    pub fn counts(&self) -> [usize; 2] {
        [self.undo.len(), self.redo.len()]
    }

    /// What undo and redo would take back next ("Paint", "Erase").
    pub(crate) fn next_labels(&self) -> [Option<&str>; 2] {
        [self.undo.last(), self.redo.last()].map(|e| e.map(|e| e.label.as_str()))
    }

    /// Record an action from its tiles' `before` states, clearing redo;
    /// returns the new blobs, for the spill.
    pub fn push(
        &mut self,
        label: impl Into<String>,
        before: Vec<(TileId, Option<Arc<TileData>>)>,
    ) -> Vec<Arc<Blob>> {
        if before.is_empty() {
            return Vec::new();
        }
        self.redo.clear();
        let mut blobs = Vec::new();
        let tiles = before
            .into_iter()
            .map(|(id, tile)| (id, snapshot(tile, &mut blobs, &self.spill)))
            .collect();
        self.undo.push(Entry { label: label.into(), tiles, applied: None });
        blobs
    }

    /// `None` when there is nothing to undo.
    pub fn undo(&mut self, canvas: &mut Canvas) -> io::Result<Option<Change>> {
        swap(&mut self.undo, &mut self.redo, &self.spill, canvas)
    }

    pub fn redo(&mut self, canvas: &mut Canvas) -> io::Result<Option<Change>> {
        swap(&mut self.redo, &mut self.undo, &self.spill, canvas)
    }

    pub(crate) fn blobs(&self) -> impl Iterator<Item = &Arc<Blob>> {
        self.undo.iter().chain(&self.redo).flat_map(Entry::blobs)
    }

    /// Bytes of the spill the stacks hold.
    pub(crate) fn live_bytes(&self) -> u64 {
        self.undo.iter().chain(&self.redo).map(Entry::stored_bytes).sum()
    }

    /// Drop the oldest undo entries until the spill fits in `cap` bytes;
    /// redo is never trimmed.
    pub fn trim(&mut self, cap: u64) {
        let mut live = self.live_bytes();
        let mut drop = 0;
        for entry in &self.undo {
            if live <= cap {
                break;
            }
            live -= entry.stored_bytes();
            drop += 1;
        }
        self.undo.drain(..drop);
    }

    /// Drop the oldest undo entries that can never leave RAM (no spill, or
    /// a failed write) until `excess` bytes are freed; returns the bytes
    /// freed. Eviction can't reach them. The newest `kept` entries stay
    /// ([`KEPT_STEPS`]); redo is never trimmed.
    pub(crate) fn shed(&mut self, excess: usize, kept: usize) -> usize {
        let no_spill = self.spill.is_none();
        let sheddable = self.undo.len().saturating_sub(kept);
        let (mut freed, mut drop) = (0, 0);
        for entry in &self.undo[..sheddable] {
            if freed >= excess || !(no_spill || entry.blobs().any(|b| b.stuck())) {
                break;
            }
            freed += entry.blobs().map(|b| b.charge()).sum::<usize>();
            drop += 1;
        }
        self.undo.drain(..drop);
        freed
    }
}

fn snapshot(
    tile: Option<Arc<TileData>>,
    blobs: &mut Vec<Arc<Blob>>,
    spill: &Option<Arc<Spill>>,
) -> Snapshot {
    match tile {
        None => Snapshot::Original,
        Some(tile) => {
            let blob = Blob::resident(tile, spill.clone());
            blobs.push(blob.clone());
            Snapshot::Blob(blob)
        }
    }
}

/// Pop an entry from `from`, swap its tiles into the canvas, and push the
/// reverse onto `to`. All tiles load first, so a failed spill read changes
/// nothing. The reverse reuses `applied`, so only a stroke's entry makes
/// new blobs.
fn swap(
    from: &mut Vec<Entry>,
    to: &mut Vec<Entry>,
    spill: &Option<Arc<Spill>>,
    canvas: &mut Canvas,
) -> io::Result<Option<Change>> {
    let Some(entry) = from.last() else { return Ok(None) };
    // Decoded in parallel; the spill's reads take turns.
    let loaded = entry
        .tiles
        .par_iter()
        .map(|(id, s)| match s {
            Snapshot::Original => Ok((*id, None)),
            Snapshot::Blob(b) => Ok((*id, Some(b.load()?))),
        })
        .collect::<io::Result<Vec<_>>>()?;
    let entry = from.pop().expect("checked above");
    let mut applied = entry.applied.map(Vec::into_iter);
    let mut new_blobs = Vec::new();
    let mut tiles = Vec::with_capacity(loaded.len());
    let mut back = Vec::with_capacity(loaded.len());
    for (id, tile) in loaded {
        let old = canvas.replace(id, tile);
        let reverse = match applied.as_mut().and_then(Iterator::next) {
            Some(kept) => kept,
            None => snapshot(old, &mut new_blobs, spill),
        };
        back.push((id, reverse));
        tiles.push(id);
    }
    let applied = Some(entry.tiles.into_iter().map(|(_, s)| s).collect());
    to.push(Entry { label: entry.label, tiles: back, applied });
    Ok(Some(Change { tiles, new_blobs }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::paint::brush::Brush;
    use crate::paint::canvas::Layer;
    use crate::paint::doc::{PaintDoc, PaintDocs};
    use crate::paint::source::SourceTable;
    use crate::paint::stroke::tests::{stack, stroke};
    use crate::{CompletedStack, RenderId};

    const PATHS: [[[f32; 2]; 2]; 4] = [
        [[10.0, 10.0], [290.0, 190.0]],
        [[150.0, 0.0], [150.0, 200.0]],
        [[0.0, 100.0], [300.0, 100.0]],
        [[40.0, 160.0], [260.0, 30.0]],
    ];

    fn spill_in(dir: &tempfile::TempDir) -> Option<Arc<Spill>> {
        Some(Spill::new(dir.path()))
    }

    fn doc(base: &Arc<CompletedStack>, spill: Option<Arc<Spill>>) -> PaintDoc {
        let mut doc = PaintDoc::new(
            Layer::empty(base.width, base.height),
            UndoStack::new(spill),
            SourceTable::default(),
        );
        doc.open(base.clone());
        doc
    }

    /// Paint stroke `i` (every third erasing) and spill its blobs.
    pub(crate) fn paint(doc: &mut PaintDoc, i: usize) {
        let brush = Brush { radius: 18.0 + 6.0 * i as f32, flow: 0.6, ..Brush::default() };
        let value = (i % 3 != 2).then_some(8000 + 7000 * (i % 8) as u16);
        let done = stroke(doc.canvas_mut().unwrap(), brush, value, &PATHS[i % PATHS.len()]);
        let blobs = doc.history.push("Brush", done.before);
        store_all(&blobs, &doc.history);
        doc.mark_changed();
    }

    /// Spill `blobs` as the store thread does.
    pub(crate) fn store_all(blobs: &[Arc<Blob>], history: &UndoStack) {
        if let Some(spill) = history.spill() {
            let _ = Blob::write_all(spill, blobs);
        }
    }

    pub(crate) fn undo_all(doc: &mut PaintDoc) {
        loop {
            let (canvas, history) = doc.parts_mut().unwrap();
            let Some(change) = history.undo(canvas).unwrap() else { break };
            store_all(&change.new_blobs, history);
        }
    }

    pub(crate) fn redo_all(doc: &mut PaintDoc) {
        loop {
            let (canvas, history) = doc.parts_mut().unwrap();
            let Some(change) = history.redo(canvas).unwrap() else { break };
            store_all(&change.new_blobs, history);
        }
    }

    pub(crate) fn flat(doc: &PaintDoc) -> Vec<u16> {
        doc.canvas().unwrap().flatten()
    }

    pub(crate) fn original(doc: &PaintDoc) -> &[u16] {
        doc.canvas().unwrap().base()
    }

    fn key(n: usize) -> RenderId {
        RenderId::for_tests(&n.to_string())
    }

    #[test]
    fn undo_all_is_the_base_and_redo_all_the_final_across_docs() {
        let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let bases = [stack(300, 200, 20000), stack(260, 130, 40000)];
        let mut docs = PaintDocs::default();
        for (n, (dir, base)) in dirs.iter().zip(&bases).enumerate() {
            docs.insert(key(n), doc(base, spill_in(dir)));
        }
        // Interleaved strokes, as when switching renders mid-session.
        for i in 0..6 {
            paint(docs.get_mut(&key(i % 2)).unwrap(), i);
        }
        // All spilled: a zero budget makes every undo read from disk.
        docs.enforce_budget(0);
        assert_eq!(docs.charged(), 0);

        for n in 0..2 {
            let doc = docs.get_mut(&key(n)).unwrap();
            let painted = flat(doc);
            assert_ne!(painted, original(doc));
            undo_all(doc);
            assert_eq!(flat(doc), original(doc), "doc {n}: undo all");
            assert!(!doc.is_painted());
            redo_all(doc);
            assert_eq!(flat(doc), painted, "doc {n}: redo all");
        }
    }

    #[test]
    fn first_paint_entries_cost_nothing() {
        let base = stack(300, 200, 20000);
        let mut doc = doc(&base, None);
        let done = stroke(doc.canvas_mut().unwrap(), Brush::default(), Some(50000), &PATHS[0]);
        let blobs = doc.history.push("Brush", done.before);
        assert!(blobs.is_empty());
        assert_eq!(doc.history.blobs().count(), 0);
        // Its undo swaps the painted tiles out to redo, held there alone.
        let (canvas, history) = doc.parts_mut().unwrap();
        let change = history.undo(canvas).unwrap().unwrap();
        assert_eq!(change.new_blobs.len(), change.tiles.len());
        assert!(change.new_blobs.iter().all(|b| b.charge() == TileData::BYTES));
    }

    /// Only a stroke's first undo snapshots tiles; later redos and undos
    /// reuse them and still come back to each step's pixels.
    #[test]
    fn undo_and_redo_again_reuse_the_snapshots_they_swapped() {
        let dir = tempfile::tempdir().unwrap();
        let base = stack(300, 200, 20000);
        let mut docs = PaintDocs::default();
        let doc = docs.insert(key(0), doc(&base, spill_in(&dir)));
        paint(doc, 0);
        let before = flat(doc);
        paint(doc, 1);
        let after = flat(doc);
        let blobs = doc.history.blobs().count();
        let step = |docs: &mut PaintDocs, undo: bool| {
            let doc = docs.get_mut(&key(0)).unwrap();
            let (canvas, history) = doc.parts_mut().unwrap();
            let change = if undo { history.undo(canvas) } else { history.redo(canvas) };
            let change = change.unwrap().expect("a step");
            store_all(&change.new_blobs, history);
            (change.tiles.len(), change.new_blobs.len(), flat(doc))
        };
        let (tiles, made, shown) = step(&mut docs, true);
        assert_eq!(shown, before);
        assert!(made > 0 && made <= tiles, "the painted tiles snapshotted once: {made}");
        let total = blobs + made;
        for round in 0..3 {
            if round == 1 {
                docs.enforce_budget(0);
                assert_eq!(docs.charged(), 0, "read back from the spill");
            }
            let (_, made, shown) = step(&mut docs, false);
            assert_eq!((made, &shown), (0, &after), "redo {round}");
            let (_, made, shown) = step(&mut docs, true);
            assert_eq!((made, &shown), (0, &before), "undo {round}");
            assert_eq!(docs.get(&key(0)).unwrap().history.blobs().count(), total);
        }
        // A new stroke drops redo; undoing it snapshots its own tiles.
        let doc = docs.get_mut(&key(0)).unwrap();
        paint(doc, 2);
        assert_eq!(doc.history.counts(), [2, 0]);
        undo_all(doc);
        assert_eq!(flat(doc), original(doc));
    }

    /// A history with no spill sheds its oldest entries, never the newest
    /// [`KEPT_STEPS`]; one with writes pending keeps every entry.
    #[test]
    fn history_that_cannot_be_written_sheds_its_oldest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let base = stack(300, 200, 20000);
        let mut docs = PaintDocs::default();
        let ram_only = docs.insert(key(0), doc(&base, None));
        for i in 0..8 {
            paint(ram_only, i);
        }
        let entries = ram_only.history.counts()[0];
        let pending = docs.insert(key(1), doc(&base, spill_in(&dir)));
        let done = stroke(pending.canvas_mut().unwrap(), Brush::default(), Some(50000), &PATHS[0]);
        pending.history.push("Brush", done.before);
        let done = stroke(pending.canvas_mut().unwrap(), Brush::default(), Some(9000), &PATHS[0]);
        let unwritten = pending.history.push("Brush", done.before);
        assert!(!unwritten.is_empty(), "snapshots waiting for the spill");

        let budget = 2 * TileData::BYTES;
        let charged = docs.enforce_budget(budget);
        assert!(charged > budget, "nothing here can be evicted: {charged}");
        docs.shed(charged - budget);
        let kept = docs.get_mut(&key(0)).unwrap().history.counts()[0];
        assert!(kept < entries, "{kept} of {entries} entries kept");
        // Over budget, no more.
        docs.shed(usize::MAX);
        assert_eq!(docs.get_mut(&key(0)).unwrap().history.counts()[0], KEPT_STEPS);
        assert_eq!(docs.get_mut(&key(1)).unwrap().history.counts()[0], 2, "pending writes stay");
        // What is left still undoes.
        undo_all(docs.get_mut(&key(0)).unwrap());
    }

    /// With one kept step, only the newest outlasts the budget, and still
    /// undoes and redoes.
    #[test]
    fn under_a_memory_limit_the_newest_step_alone_is_kept() {
        let base = stack(300, 200, 20000);
        let mut doc = doc(&base, None);
        for i in 0..6 {
            paint(&mut doc, i);
        }
        let painted = flat(&doc);
        let charged: usize = doc.history.blobs().map(|b| b.charge()).sum();
        let freed = doc.history.shed(usize::MAX, 1);
        assert_eq!(doc.history.counts(), [1, 0]);
        assert!(freed > 0 && freed < charged, "{freed} of {charged}");
        let (canvas, history) = doc.parts_mut().unwrap();
        history.undo(canvas).unwrap().expect("the newest step");
        assert_eq!(doc.history.counts(), [0, 1]);
        assert_eq!(doc.history.shed(usize::MAX, 1), 0, "redo is never trimmed");
        redo_all(&mut doc);
        assert_eq!(flat(&doc), painted);
        if cfg!(target_os = "ios") {
            assert_eq!(KEPT_STEPS, 1);
        }
    }

    #[test]
    fn the_budget_is_respected() {
        let dir = tempfile::tempdir().unwrap();
        let base = stack(300, 200, 20000);
        let mut docs = PaintDocs::default();
        let doc = docs.insert(key(0), doc(&base, spill_in(&dir)));
        for i in 0..8 {
            paint(doc, i);
        }
        let charged = docs.charged();
        assert!(charged > 4 * TileData::BYTES, "{charged}");
        let budget = 3 * TileData::BYTES;
        assert!(docs.enforce_budget(budget) <= budget);
        assert!(docs.charged() <= budget);
        // The most recently used blobs are the ones kept.
        let doc = docs.get_mut(&key(0)).unwrap();
        let painted = flat(doc);
        undo_all(doc);
        assert_eq!(flat(doc), original(doc));
        redo_all(doc);
        assert_eq!(flat(doc), painted);
    }

    /// A stroke that changes no pixel (a hard brush over its own paint)
    /// adds no undo entry and leaves redo as it was.
    #[test]
    fn a_stroke_that_changes_nothing_keeps_undo_and_redo() {
        let base = stack(300, 200, 20000);
        let mut doc = doc(&base, None);
        let hard = Brush { radius: 30.0, hardness: 1.0, ..Brush::default() };
        let (a, b) = ([[40.0, 40.0], [260.0, 60.0]], [[150.0, 20.0], [150.0, 180.0]]);
        for path in [a, b] {
            let done = stroke(doc.canvas_mut().unwrap(), hard, Some(50000), &path);
            doc.history.push("Brush", done.before);
        }
        let (canvas, history) = doc.parts_mut().unwrap();
        history.undo(canvas).unwrap();
        assert_eq!(doc.history.counts(), [1, 1]);
        let shown = flat(&doc);
        let done = stroke(doc.canvas_mut().unwrap(), hard, Some(50000), &a);
        assert!(doc.history.push("Brush", done.before).is_empty());
        assert_eq!(doc.history.counts(), [1, 1], "no entry, and redo kept");
        assert_eq!(flat(&doc), shown);
        redo_all(&mut doc);
        assert_eq!(doc.history.counts(), [2, 0]);
    }

    #[test]
    fn trim_drops_the_oldest_entries_until_the_spill_fits() {
        let dir = tempfile::tempdir().unwrap();
        let base = stack(300, 200, 20000);
        let mut doc = doc(&base, spill_in(&dir));
        for i in 0..8 {
            paint(&mut doc, i);
        }
        let live = doc.history.live_bytes();
        let undo = doc.history.counts()[0];
        doc.history.trim(live);
        assert_eq!(doc.history.counts()[0], undo, "it fits already");
        doc.history.trim(live / 3);
        let [kept, _] = doc.history.counts();
        assert!(kept < undo && kept > 0, "{kept} of {undo}");
        assert!(doc.history.live_bytes() <= live / 3);
        // The newest entries are kept.
        let painted = flat(&doc);
        undo_all(&mut doc);
        redo_all(&mut doc);
        assert_eq!(flat(&doc), painted);
        doc.history.trim(0);
        assert_eq!(doc.history.counts()[0], 0);
        // The rows of the entries dropped go with the spill's next write.
        paint(&mut doc, 8);
        let file = rusqlite::Connection::open(dir.path().join(crate::paint::store::UNDO_FILE));
        let rows: i64 =
            file.unwrap().query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0)).unwrap();
        assert_eq!(rows as usize, doc.history.blobs().filter(|b| b.stored_len().is_some()).count());
    }

    /// Every blob that isn't written still has its tile in RAM.
    fn none_lost(docs: &PaintDocs, at: &str) {
        for blob in docs.get(&key(0)).unwrap().history.blobs() {
            let state = lock(&blob.state);
            assert!(state.tile().is_some() || state.row().is_some(), "{at}: a tile in neither");
            if let BlobState::Evicted { .. } = *state {
                continue;
            }
            assert!(state.tile().is_some(), "{at}: {} with no tile", state.name());
        }
    }

    /// How the spill's `n`-th snapshot write fails.
    #[derive(Clone, Copy, Debug)]
    enum Failing {
        /// That write alone.
        Fault,
        /// That one and every one after, as a disk that filled.
        Full,
    }

    /// Paint, undo and redo with the spill failing at write `n` and a zero
    /// budget after every step; returns the writes made.
    fn evicting(failing: Option<(usize, Failing)>) -> usize {
        let dir = tempfile::tempdir().unwrap();
        let base = stack(300, 200, 20000);
        let mut docs = PaintDocs::default();
        let spill = Spill::new(dir.path());
        let writes = Arc::new(AtomicU64::new(0));
        let counted = writes.clone();
        *lock(&spill.before_write) = Some(Box::new(move || {
            let n = counted.fetch_add(1, Ordering::Relaxed) as usize + 1;
            match failing {
                Some((at, Failing::Fault)) if n == at => Err(full()),
                Some((at, Failing::Full)) if n >= at => Err(full()),
                _ => Ok(()),
            }
        }));
        docs.insert(key(0), doc(&base, Some(spill)));
        let at = format!("{failing:?}");
        let evict = |docs: &PaintDocs, step: &str| {
            let unwritten: usize = docs
                .get(&key(0))
                .unwrap()
                .history
                .blobs()
                .filter(|b| !b.evictable())
                .map(|b| b.charge())
                .sum();
            assert_eq!(docs.enforce_budget(0), unwritten, "{at}, {step}: all else evicted");
            none_lost(docs, &format!("{at}, {step}"));
        };
        let mut steps = vec![flat(docs.get(&key(0)).unwrap())];
        for i in 0..6 {
            paint(docs.get_mut(&key(0)).unwrap(), i);
            evict(&docs, &format!("stroke {i}"));
            steps.push(flat(docs.get(&key(0)).unwrap()));
        }
        for (n, expected) in steps.iter().enumerate().rev().skip(1) {
            let doc = docs.get_mut(&key(0)).unwrap();
            let (canvas, history) = doc.parts_mut().unwrap();
            let change = history.undo(canvas).unwrap().expect("a step to undo");
            store_all(&change.new_blobs, history);
            assert_eq!(&flat(doc), expected, "{at}: undone to {n}");
            evict(&docs, &format!("undo to {n}"));
        }
        for (n, expected) in steps.iter().enumerate().skip(1) {
            let doc = docs.get_mut(&key(0)).unwrap();
            let (canvas, history) = doc.parts_mut().unwrap();
            let change = history.redo(canvas).unwrap().expect("a step to redo");
            store_all(&change.new_blobs, history);
            assert_eq!(&flat(doc), expected, "{at}: redone to {n}");
            evict(&docs, &format!("redo to {n}"));
        }
        writes.load(Ordering::Relaxed) as usize
    }

    fn full() -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL), None)
    }

    /// A resident tile is dropped only once its write is confirmed.
    #[test]
    fn a_tile_leaves_ram_only_once_written() {
        let writes = evicting(None);
        assert!(writes > 6, "{writes}");
        use rayon::prelude::*;
        (1..=writes).into_par_iter().for_each(|n| {
            evicting(Some((n, Failing::Fault)));
            evicting(Some((n, Failing::Full)));
        });
    }

    /// Eviction at every point of a concurrent spill write (`shuttle`)
    /// keeps what is unwritten.
    #[test]
    fn eviction_while_the_store_writes_keeps_what_is_unwritten() {
        let mut config = shuttle::Config::new();
        config.stack_size = 8 << 20;
        let scheduler = shuttle::scheduler::DfsScheduler::new(None, false);
        let explored = shuttle::Runner::new(scheduler, config).run(|| {
            let dir = tempfile::tempdir().unwrap();
            let base = stack(300, 200, 20000);
            let mut docs = PaintDocs::default();
            let spill = Spill::new(dir.path());
            let doc = docs.insert(key(0), doc(&base, Some(spill.clone())));
            paint(doc, 0);
            let painted = flat(doc);
            let done = stroke(doc.canvas_mut().unwrap(), Brush::default(), Some(50000), &PATHS[1]);
            let blobs = doc.history.push("Brush", done.before);
            assert!(blobs.len() > 1, "a stroke over several painted tiles");
            let stroked = flat(doc);
            *lock(&spill.before_write) = Some(Box::new(|| {
                shuttle::thread::yield_now();
                Ok(())
            }));
            let store = shuttle::thread::spawn(move || {
                shuttle::thread::yield_now();
                Blob::write_all(&spill, &blobs).unwrap();
                shuttle::thread::yield_now();
            });
            for _ in 0..4 {
                docs.enforce_budget(0);
                none_lost(&docs, "while storing");
                shuttle::thread::yield_now();
            }
            store.join().unwrap();
            assert_eq!(docs.enforce_budget(0), 0, "all written, all evicted");
            let doc = docs.get_mut(&key(0)).unwrap();
            let (canvas, history) = doc.parts_mut().unwrap();
            history.undo(canvas).unwrap().expect("the stroke");
            assert_eq!(flat(doc), painted);
            let (canvas, history) = doc.parts_mut().unwrap();
            history.redo(canvas).unwrap().expect("the stroke");
            assert_eq!(flat(doc), stroked);
        });
        assert!(explored > 1, "{explored}");
    }
}
