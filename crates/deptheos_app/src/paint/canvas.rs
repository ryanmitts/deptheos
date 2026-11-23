//! The painted result: a grid of copy-on-write tiles over the original.

use std::sync::Arc;

use rayon::prelude::*;

use super::tile::{TILE, TileData, TileId};
use crate::CompletedStack;
use crate::engine::present::PixelRect;

/// A render's painted pixels. Unpainted tiles are `None` and read through
/// to the original, so a canvas costs memory only where it is painted.
pub struct Canvas {
    base: Arc<CompletedStack>,
    /// Of the base's size.
    layer: Layer,
}

/// A canvas's painted tiles without the original under them: what a doc
/// keeps while its render is not open.
#[derive(Clone)]
pub struct Layer {
    pub width: u32,
    pub height: u32,
    /// In grid order; `None` = the original's.
    pub tiles: Vec<Option<Arc<TileData>>>,
}

impl Layer {
    /// Nothing painted on a `width`×`height` render.
    pub fn empty(width: u32, height: u32) -> Self {
        let n = width.div_ceil(TILE) * height.div_ceil(TILE);
        Self { width, height, tiles: vec![None; n as usize] }
    }

    /// The grid size, in tiles.
    pub fn grid(&self) -> [u32; 2] {
        [self.width.div_ceil(TILE), self.height.div_ceil(TILE)]
    }

    /// Every painted tile.
    pub fn painted(&self) -> impl Iterator<Item = (TileId, &Arc<TileData>)> {
        let cols = self.grid()[0];
        self.tiles.iter().enumerate().filter_map(move |(i, tile)| {
            let i = i as u32;
            tile.as_ref().map(|t| (TileId { x: i % cols, y: i / cols }, t))
        })
    }

    fn index(&self, id: TileId) -> usize {
        (id.y * self.grid()[0] + id.x) as usize
    }
}

impl Canvas {
    /// An unpainted canvas over `base`.
    pub fn new(base: Arc<CompletedStack>) -> Self {
        let layer = Layer::empty(base.width, base.height);
        Self::with_layer(base, layer)
    }

    /// `layer` over `base`.
    ///
    /// # Panics
    /// When the layer was painted on a render of another size.
    pub(crate) fn with_layer(base: Arc<CompletedStack>, layer: Layer) -> Self {
        assert_eq!([layer.width, layer.height], [base.width, base.height], "layer size");
        Self { base, layer }
    }

    /// The painted tiles, letting go of the original.
    pub(crate) fn into_layer(self) -> Layer {
        self.layer
    }

    /// A copy of the painted tiles (sharing them).
    pub fn layer(&self) -> Layer {
        self.layer.clone()
    }

    pub fn width(&self) -> u32 {
        self.base.width
    }

    pub fn height(&self) -> u32 {
        self.base.height
    }

    /// The grid size, in tiles.
    pub fn grid(&self) -> [u32; 2] {
        self.layer.grid()
    }

    /// The original result, as interleaved RGB.
    pub fn base(&self) -> &[u16] {
        &self.base.pixels
    }

    /// Tile `id`, or `None` when it is the original's.
    pub fn tile(&self, id: TileId) -> Option<&Arc<TileData>> {
        self.layer.tiles[self.layer.index(id)].as_ref()
    }

    /// Put `tile` at `id` and hand back what was there. Undo and redo are
    /// this swap, so they copy nothing.
    pub fn replace(&mut self, id: TileId, tile: Option<Arc<TileData>>) -> Option<Arc<TileData>> {
        let i = self.layer.index(id);
        std::mem::replace(&mut self.layer.tiles[i], tile)
    }

    /// Every painted tile.
    pub fn painted(&self) -> impl Iterator<Item = (TileId, &Arc<TileData>)> {
        self.layer.painted()
    }

    /// The canvas tiles `rect` (image pixels) touches, row by row.
    pub(crate) fn tiles_in(&self, rect: PixelRect) -> impl Iterator<Item = TileId> + use<> {
        let (cols, rows) = Self::tile_span(rect);
        rows.flat_map(move |y| cols.clone().map(move |x| TileId { x, y }))
    }

    /// The grid columns and rows, in tiles, that `rect` (image pixels, not
    /// empty) touches.
    pub(crate) fn tile_span(
        rect: PixelRect,
    ) -> (std::ops::RangeInclusive<u32>, std::ops::RangeInclusive<u32>) {
        let (x0, x1) = (rect.x / TILE, (rect.right() - 1) / TILE);
        let (y0, y1) = (rect.y / TILE, (rect.bottom() - 1) / TILE);
        (x0..=x1, y0..=y1)
    }

    /// The full painted result as interleaved RGB: one transient copy, for
    /// saves, exports and use as a source.
    pub fn flatten(&self) -> Vec<u16> {
        self.read(PixelRect { x: 0, y: 0, width: self.width(), height: self.height() })
    }

    /// The painted pixels of `rect` (image pixels), as interleaved RGB.
    pub fn read(&self, rect: PixelRect) -> Vec<u16> {
        let (width, row) = (self.width() as usize, 3 * rect.width as usize);
        let mut out = vec![0; row * rect.height as usize];
        out.par_chunks_mut(row).enumerate().for_each(|(i, dst)| {
            let y = rect.y + i as u32;
            let mut x = rect.x;
            while x < rect.right() {
                let id = TileId { x: x / TILE, y: y / TILE };
                let run = (TILE - x % TILE).min(rect.right() - x) as usize;
                let at = 3 * (x - rect.x) as usize;
                let src = match self.tile(id) {
                    Some(tile) => {
                        let (lx, ly) = ((x % TILE) as usize, (y % TILE) as usize);
                        &tile.rgb[3 * (ly * TILE as usize + lx)..]
                    }
                    None => &self.base.pixels[3 * (y as usize * width + x as usize)..],
                };
                dst[at..at + 3 * run].copy_from_slice(&src[..3 * run]);
                x += run as u32;
            }
        });
        out
    }

    /// Mutable access to the tiles of grid rows `rows`, columns `cols`,
    /// for a stroke to paint. Each slice is one grid row.
    pub(crate) fn rows_mut(
        &mut self,
        rows: std::ops::RangeInclusive<u32>,
        cols: std::ops::RangeInclusive<u32>,
    ) -> impl Iterator<Item = (u32, &mut [Option<Arc<TileData>>])> {
        let n = self.grid()[0] as usize;
        let (c0, c1) = (*cols.start() as usize, *cols.end() as usize);
        let y0 = *rows.start();
        self.layer
            .tiles
            .chunks_mut(n)
            .skip(y0 as usize)
            .take(rows.count())
            .enumerate()
            .map(move |(i, row)| (y0 + i as u32, &mut row[c0..=c1]))
    }

    /// The stack the canvas paints over.
    pub fn stack(&self) -> &Arc<CompletedStack> {
        &self.base
    }

    /// The original's tile `id`.
    pub(crate) fn base_tile(&self, id: TileId) -> TileData {
        TileData::from_base(&self.base.pixels, self.width(), self.height(), id)
    }

    /// Whether tile `id`'s pixels equal the original's.
    pub(crate) fn is_original(&self, id: TileId, tile: &TileData) -> bool {
        tile.matches_base(&self.base.pixels, self.width(), self.height(), id)
    }
}
