//! One stroke: dabs composited onto the canvas from the stroke's start
//! state through a wash-style coverage mask, so overlapping dabs never
//! build up past the opacity and every pixel is rounded once.

use std::collections::BTreeSet;
use std::sync::Arc;

use rayon::prelude::*;

use super::brush::{Brush, Dab, tolerance};
use super::canvas::Canvas;
use super::tile::{ORIGINAL, TILE, TILE_PIXELS, TileData, TileId};
use crate::engine::present::PixelRect;

/// Dabs covering more pixels than this split across threads.
const PARALLEL_PIXELS: u32 = 1 << 16;

/// Where a stroke takes its pixels from.
#[derive(Clone)]
pub enum StrokeSource {
    /// The original result: the eraser.
    Original,
    /// Interleaved RGB in the canvas's space; `id` is its source table
    /// index.
    Image { id: u8, pixels: Arc<Vec<u16>> },
}

/// Coverage scratch buffers, reused from stroke to stroke.
#[derive(Default)]
pub struct CoveragePool(Vec<Box<[f32]>>);

impl CoveragePool {
    fn take(&mut self) -> Box<[f32]> {
        match self.0.pop() {
            Some(mut c) => {
                c.fill(0.0);
                c
            }
            None => vec![0.0; TILE_PIXELS].into_boxed_slice(),
        }
    }
}

/// A touched tile's state for the stroke.
struct Touched {
    /// The tile as found (`None`: the original): every dab's base, and the
    /// undo snapshot.
    before: Option<Arc<TileData>>,
    /// The stroke's coverage so far.
    c: Box<[f32]>,
}

/// A stroke in progress.
pub struct Stroke {
    brush: Brush,
    source: StrokeSource,
    /// Coverage each dab adds below flow 1 ([`Brush::flow_per_dab`]).
    phi: f32,
    /// Grid columns, to index `touched`.
    cols: u32,
    /// Per canvas tile, in grid order.
    touched: Vec<Option<Touched>>,
    /// The touched tiles, in the order they were first touched.
    order: Vec<TileId>,
    /// Tiles changed since the last [`Self::take_dirty`].
    dirty: BTreeSet<TileId>,
    pool: CoveragePool,
}

/// What a finished stroke changed.
pub struct FinishedStroke {
    /// Each changed tile as it was: the undo entry.
    pub before: Vec<(TileId, Option<Arc<TileData>>)>,
    /// The scratch buffers, for the next stroke.
    pub pool: CoveragePool,
}

impl Stroke {
    pub fn new(canvas: &Canvas, brush: Brush, source: StrokeSource, pool: CoveragePool) -> Self {
        let [cols, rows] = canvas.grid();
        let mut touched = Vec::new();
        touched.resize_with((cols * rows) as usize, || None);
        let phi = brush.flow_per_dab();
        Self { brush, source, phi, cols, touched, order: Vec::new(), dirty: BTreeSet::new(), pool }
    }

    /// The tiles changed since the last call.
    pub(crate) fn take_dirty(&mut self) -> BTreeSet<TileId> {
        std::mem::take(&mut self.dirty)
    }

    /// Stamp one dab.
    pub fn dab(&mut self, canvas: &mut Canvas, dab: Dab) {
        let r = self.brush.radius;
        let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
        let (x0, x1) =
            (clamp((dab.x - r).floor(), canvas.width()), clamp((dab.x + r).ceil(), canvas.width()));
        let (y0, y1) = (
            clamp((dab.y - r).floor(), canvas.height()),
            clamp((dab.y + r).ceil(), canvas.height()),
        );
        if x0 == x1 || y0 == y1 {
            return;
        }
        let rect = PixelRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
        for id in canvas.tiles_in(rect) {
            self.touch(canvas, id);
            self.dirty.insert(id);
        }

        let stack = canvas.stack().clone();
        let (width, cols) = (stack.width as usize, canvas.grid()[0] as usize);
        let (tile_cols, tile_rows) = Canvas::tile_span(rect);
        let (tx0, tx1) = (*tile_cols.start(), *tile_cols.end());
        let touched = self.touched.chunks_mut(cols).skip(*tile_rows.start() as usize);
        let mut jobs = Vec::new();
        for ((ty, tiles), touched) in canvas.rows_mut(tile_rows, tile_cols).zip(touched) {
            let touched = &mut touched[tx0 as usize..=tx1 as usize];
            for (tx, (tile, t)) in (tx0..).zip(tiles.iter_mut().zip(touched)) {
                let tile = Arc::make_mut(tile.as_mut().expect("a touched tile is painted"));
                jobs.push((TileId { x: tx, y: ty }, tile, t.as_mut().expect("touched above")));
            }
        }
        let paint = Paint {
            brush: &self.brush,
            phi: self.phi,
            base: &stack.pixels,
            width,
            source: match &self.source {
                StrokeSource::Original => None,
                StrokeSource::Image { id, pixels } => Some((*id, pixels.as_slice())),
            },
            dab,
            rect,
        };
        if rect.width * rect.height > PARALLEL_PIXELS {
            jobs.into_par_iter().for_each(|(id, tile, t)| paint.tile(id, tile, t));
        } else {
            jobs.into_iter().for_each(|(id, tile, t)| paint.tile(id, tile, t));
        }
    }

    /// End the stroke. Tiles equal to the original go back to `None`;
    /// unchanged tiles get their old `Arc` back and stay out of the undo
    /// entry.
    pub fn finish(mut self, canvas: &mut Canvas) -> FinishedStroke {
        let cols = canvas.grid()[0];
        let mut before = Vec::with_capacity(self.order.len());
        for id in self.order {
            let t = self.touched[(id.y * cols + id.x) as usize].take().expect("touched");
            self.pool.0.push(t.c);
            let tile = canvas.tile(id).expect("a touched tile is painted");
            if let Some(old) = t.before.as_ref().filter(|old| ***old == **tile) {
                // The same `Arc`, so the store sees it as already saved.
                canvas.replace(id, Some(old.clone()));
                continue;
            }
            if canvas.is_original(id, tile) {
                canvas.replace(id, None);
                // Original before and after: nothing to undo.
                if t.before.is_none() {
                    continue;
                }
            }
            before.push((id, t.before));
        }
        FinishedStroke { before, pool: self.pool }
    }

    /// The undo entry of a stroke cut short by a panic, tiles left as they
    /// are.
    pub(crate) fn abandon(mut self) -> FinishedStroke {
        let cols = self.cols;
        let before = self
            .order
            .iter()
            .filter_map(|id| {
                let t = self.touched.get_mut((id.y * cols + id.x) as usize)?.take()?;
                Some((*id, t.before))
            })
            .collect();
        FinishedStroke { before, pool: CoveragePool::default() }
    }

    /// Snapshot tile `id` and give the canvas its own copy to paint.
    fn touch(&mut self, canvas: &mut Canvas, id: TileId) {
        let i = (id.y * canvas.grid()[0] + id.x) as usize;
        if self.touched[i].is_some() {
            return;
        }
        let before = canvas.tile(id).cloned();
        let tile = match &before {
            Some(tile) => TileData::clone(tile),
            None => canvas.base_tile(id),
        };
        canvas.replace(id, Some(Arc::new(tile)));
        self.touched[i] = Some(Touched { before, c: self.pool.take() });
        self.order.push(id);
    }
}

/// One dab's inputs, shared by the tiles it paints.
struct Paint<'a> {
    brush: &'a Brush,
    phi: f32,
    /// The original result, as interleaved RGB.
    base: &'a [u16],
    /// The image width, in pixels.
    width: usize,
    source: Option<(u8, &'a [u16])>,
    dab: Dab,
    /// The pixels the dab can reach, in image pixels.
    rect: PixelRect,
}

impl Paint<'_> {
    /// `gx`, `gy` are image pixels; `i` indexes the pixel in the tile.
    fn tile(&self, id: TileId, tile: &mut TileData, t: &mut Touched) {
        let (ox, oy) = (id.x * TILE, id.y * TILE);
        let (xa, xb) = (self.rect.x.max(ox), self.rect.right().min(ox + TILE));
        let (ya, yb) = (self.rect.y.max(oy), self.rect.bottom().min(oy + TILE));
        let brush = self.brush;
        for gy in ya..yb {
            let dy = gy as f32 + 0.5 - self.dab.y;
            for gx in xa..xb {
                let dx = gx as f32 + 0.5 - self.dab.x;
                let f = brush.falloff(dx.hypot(dy));
                if f == 0.0 {
                    continue;
                }
                let i = ((gy - oy) * TILE + (gx - ox)) as usize;
                let fp = f * self.dab.pressure;
                // At flow 1 a pixel keeps the strongest dab's profile, so
                // the hardness survives overlapping dabs; below it, each
                // dab adds `phi` of what is left.
                let c = &mut t.c[i];
                *c = if brush.flow == 1.0 { c.max(fp) } else { *c + (1.0 - *c) * fp * self.phi };

                let at = 3 * (gy as usize * self.width + gx as usize);
                let original: [u16; 3] = self.base[at..at + 3].try_into().unwrap();
                let (sid, src) = match self.source {
                    Some((sid, pixels)) => (sid, pixels[at..at + 3].try_into().unwrap()),
                    None => (ORIGINAL, original),
                };
                let tol = brush.tolerance.map_or(1.0, |tol| tolerance(tol, src, original));
                let alpha = brush.opacity * *c * tol;

                let (before, cov_before, sid_before) = match &t.before {
                    Some(b) => {
                        (b.rgb[3 * i..3 * i + 3].try_into().unwrap(), b.coverage[i], b.source[i])
                    }
                    None => (original, 0, ORIGINAL),
                };
                for ch in 0..3 {
                    let (b, s) = (f32::from(before[ch]), f32::from(src[ch]));
                    tile.rgb[3 * i + ch] = ((b + (s - b) * alpha).clamp(0.0, 65535.0) + 0.5) as u16;
                }
                let cov_before = f32::from(cov_before) / 255.0;
                let cov = match self.source {
                    Some(_) => 1.0 - (1.0 - cov_before) * (1.0 - alpha),
                    None => cov_before * (1.0 - alpha),
                };
                tile.coverage[i] = (cov * 255.0 + 0.5) as u8;
                // The source switches once the stroke is the larger share.
                tile.source[i] = if alpha >= 0.5 { sid } else { sid_before };
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::CompletedStack;
    use crate::paint::brush::Spacer;

    pub(crate) fn canvas(width: u32, height: u32, value: u16) -> Canvas {
        Canvas::new(stack(width, height, value))
    }

    /// An sRGB stack of `value` plus a little texture.
    pub(crate) fn stack(width: u32, height: u32, value: u16) -> Arc<CompletedStack> {
        let pixels = (0..3 * width * height).map(|i| value.wrapping_add((i % 7) as u16)).collect();
        Arc::new(CompletedStack::srgb(width, height, pixels, Vec::new()))
    }

    fn flat(canvas: &Canvas, value: u16) -> StrokeSource {
        StrokeSource::Image { id: 1, pixels: Arc::new(vec![value; canvas.base().len()]) }
    }

    /// Stroke `path` from a flat `source` value, or erase when it is `None`.
    pub(crate) fn stroke(
        canvas: &mut Canvas,
        brush: Brush,
        source: Option<u16>,
        path: &[[f32; 2]],
    ) -> FinishedStroke {
        let source = source.map_or(StrokeSource::Original, |v| flat(canvas, v));
        let mut stroke = Stroke::new(canvas, brush, source, CoveragePool::default());
        let mut spacer = Spacer::new(&brush);
        let mut dabs = Vec::new();
        for &[x, y] in path {
            spacer.to(Dab { x, y, pressure: 1.0 }, &mut dabs);
        }
        for dab in dabs {
            stroke.dab(canvas, dab);
        }
        stroke.finish(canvas)
    }

    fn red(canvas: &Canvas, x: u32, y: u32) -> u16 {
        canvas.read(PixelRect { x, y, width: 1, height: 1 })[0]
    }

    #[test]
    fn an_empty_canvas_flattens_to_the_base() {
        let canvas = canvas(300, 200, 1000);
        assert_eq!(canvas.flatten(), canvas.base());
        assert_eq!(canvas.painted().count(), 0);
    }

    #[test]
    fn a_rect_reads_like_that_rect_of_the_flattened_canvas() {
        let mut canvas = canvas(300, 200, 1000);
        let brush = Brush { radius: 40.0, ..Brush::default() };
        stroke(&mut canvas, brush, Some(50000), &[[100.0, 100.0], [290.0, 190.0]]);
        let full = canvas.flatten();
        let rect = PixelRect { x: 77, y: 51, width: 223, height: 149 };
        let crop: Vec<u16> = (rect.y..rect.bottom())
            .flat_map(|y| &full[3 * (y * 300 + rect.x) as usize..][..3 * rect.width as usize])
            .copied()
            .collect();
        assert_eq!(canvas.read(rect), crop);
    }

    #[test]
    fn hardness_survives_at_flow_1() {
        // Along a straight stroke, a pixel a distance d from the line gets
        // the profile of the nearest dab, not the build-up of the ~8 dabs
        // that cover it (source-over would take 0.32 to ~0.9 here).
        let mut canvas = canvas(512, 96, 0);
        let brush = Brush { radius: 20.0, hardness: 0.5, ..Brush::default() };
        stroke(&mut canvas, brush, Some(65535), &[[40.0, 48.0], [470.0, 48.0]]);
        let d = 16.5; // row 64's centers
        let half = brush.spacing() / 2.0;
        let (most, least) = (brush.falloff(d), brush.falloff(d.hypot(half)));
        for x in (100..400).step_by(7) {
            let v = f32::from(red(&canvas, x, 64)) / 65535.0;
            assert!(v >= least - 1e-3 && v <= most + 1e-3, "x {x}: {v} not in {least}..{most}");
        }
        assert!(most < 0.35);
    }

    #[test]
    fn opacity_caps_the_stroke() {
        for (flow, hardness) in [(1.0, 1.0), (0.2, 0.5)] {
            let mut canvas = canvas(256, 256, 0);
            let brush = Brush { radius: 30.0, hardness, opacity: 0.5, flow, ..Brush::default() };
            let mut stroke =
                Stroke::new(&canvas, brush, flat(&canvas, 65535), CoveragePool::default());
            for _ in 0..200 {
                stroke.dab(&mut canvas, Dab { x: 128.0, y: 128.0, pressure: 1.0 });
            }
            stroke.finish(&mut canvas);
            let half = |b: u16| ((f32::from(b) + (65535.0 - f32::from(b)) * 0.5) + 0.5) as u16;
            let pixels = canvas.flatten();
            assert!(
                pixels.iter().zip(canvas.base()).all(|(&v, &b)| v <= half(b)),
                "flow {flow} went past the opacity"
            );
            let center = half(canvas.base()[3 * (128 * 256 + 128)]);
            assert!(red(&canvas, 128, 128) >= center - 100, "flow {flow} never reached it");
        }
    }

    #[test]
    fn tolerance_is_tested_against_the_original() {
        let mut canvas = canvas(256, 256, 30000);
        let hard = Brush { radius: 50.0, hardness: 1.0, ..Brush::default() };
        let path = [[128.0, 128.0]];
        stroke(&mut canvas, hard, Some(50000), &path);
        assert_eq!(red(&canvas, 128, 128), 50000);
        let tolerant = Brush { tolerance: Some(0.1), ..hard };
        // Near the painted pixels but far from the original: not painted.
        stroke(&mut canvas, tolerant, Some(49000), &path);
        assert_eq!(red(&canvas, 128, 128), 50000);
        // Far from the painted pixels but near the original: painted.
        stroke(&mut canvas, tolerant, Some(31000), &path);
        assert_eq!(red(&canvas, 128, 128), 31000);
    }

    /// A stroke that changes nothing has no undo entry and keeps the very
    /// tiles.
    #[test]
    fn a_stroke_that_changes_nothing_records_nothing() {
        let mut canvas = canvas(300, 200, 30000);
        let hard = Brush { radius: 40.0, hardness: 1.0, ..Brush::default() };
        let path = [[60.0, 60.0], [240.0, 140.0]];
        let first = stroke(&mut canvas, hard, Some(50000), &path);
        assert!(!first.before.is_empty());
        let tiles: Vec<_> = canvas.painted().map(|(id, t)| (id, Arc::as_ptr(t))).collect();
        let again = stroke(&mut canvas, hard, Some(50000), &path);
        assert!(again.before.is_empty(), "{} tiles recorded", again.before.len());
        let tolerant = Brush { tolerance: Some(0.1), ..hard };
        let refused = stroke(&mut canvas, tolerant, Some(49000), &path);
        assert!(refused.before.is_empty());
        let after: Vec<_> = canvas.painted().map(|(id, t)| (id, Arc::as_ptr(t))).collect();
        assert_eq!(after, tiles, "the same tiles, not copies");
    }

    #[test]
    fn erasing_collapses_tiles() {
        let mut canvas = canvas(300, 200, 1000);
        let brush = Brush { radius: 40.0, hardness: 0.3, flow: 0.4, ..Brush::default() };
        let path = [[0.0, 0.0], [150.0, 100.0], [300.0, 200.0]];
        let first = stroke(&mut canvas, brush, Some(50000), &path);
        let painted = canvas.painted().count();
        assert!(painted > 1);
        assert_eq!(first.before.len(), painted);
        assert!(first.before.iter().all(|(_, b)| b.is_none()), "first paint snapshots nothing");
        let tile = canvas.painted().next().unwrap().1.clone();
        assert!(tile.coverage.iter().any(|&c| c > 0) && tile.source.contains(&1));

        let eraser = Brush { radius: 80.0, hardness: 1.0, ..Brush::default() };
        let erased = stroke(&mut canvas, eraser, None, &path);
        assert_eq!(canvas.painted().count(), 0);
        assert_eq!(canvas.flatten(), canvas.base());
        assert_eq!(erased.before.len(), painted);
        assert!(erased.before.iter().all(|(_, b)| b.is_some()));
    }

    #[test]
    fn an_abandoned_stroke_still_has_its_undo_snapshots() {
        let mut canvas = canvas(300, 200, 1000);
        let brush = Brush { radius: 25.0, ..Brush::default() };
        stroke(&mut canvas, brush, Some(50000), &[[20.0, 20.0], [280.0, 40.0]]);
        let before = canvas.flatten();
        let mut cut = Stroke::new(&canvas, brush, flat(&canvas, 9000), CoveragePool::default());
        cut.dab(&mut canvas, Dab { x: 150.0, y: 100.0, pressure: 1.0 });
        cut.dab(&mut canvas, Dab { x: 250.0, y: 30.0, pressure: 1.0 });
        let abandoned = cut.abandon();
        assert!(!abandoned.before.is_empty());
        assert_ne!(canvas.flatten(), before);
        for (id, tile) in abandoned.before {
            canvas.replace(id, tile);
        }
        assert_eq!(canvas.flatten(), before);
    }

    #[test]
    fn undoing_a_stroke_is_swapping_its_snapshots_back() {
        let mut canvas = canvas(300, 200, 1000);
        let brush = Brush { radius: 25.0, flow: 0.5, ..Brush::default() };
        stroke(&mut canvas, brush, Some(50000), &[[20.0, 20.0], [280.0, 40.0]]);
        let after_first = canvas.flatten();
        let second = stroke(&mut canvas, brush, Some(9000), &[[150.0, 0.0], [150.0, 200.0]]);
        assert_ne!(canvas.flatten(), after_first);
        for (id, before) in second.before {
            canvas.replace(id, before);
        }
        assert_eq!(canvas.flatten(), after_first);
    }
}
