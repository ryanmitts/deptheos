//! One painted tile of a canvas: its pixels, how far they have moved from
//! the original, and which source put them there.

use crate::engine::present::PixelRect;

/// Side of a canvas tile, in image pixels.
pub const TILE: u32 = 128;

/// Pixels in a tile. Edge tiles are stored whole; the pixels past the
/// image edge stay zero.
pub const TILE_PIXELS: usize = (TILE * TILE) as usize;

/// The source id of the original result: unpainted pixels, and pixels the
/// eraser restored.
pub(crate) const ORIGINAL: u8 = 0;

/// A tile's position in the canvas grid, in tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub x: u32,
    pub y: u32,
}

/// A painted tile, [`TileData::BYTES`] (8 bytes per pixel).
#[derive(Clone, PartialEq, Eq)]
pub struct TileData {
    /// Interleaved RGB, in the result's own space.
    pub rgb: Box<[u16]>,
    /// How far each pixel has moved from the original, as an alpha.
    pub coverage: Box<[u8]>,
    /// The dominant source of each pixel's last paint, an index into the
    /// doc's source table ([`ORIGINAL`] for none).
    pub source: Box<[u8]>,
}

impl TileData {
    /// All zero, from the original source.
    pub fn blank() -> Self {
        Self {
            rgb: vec![0; 3 * TILE_PIXELS].into_boxed_slice(),
            coverage: vec![0; TILE_PIXELS].into_boxed_slice(),
            source: vec![ORIGINAL; TILE_PIXELS].into_boxed_slice(),
        }
    }

    /// Tile `id` of the original: `base` is the `width`-pixel-wide result
    /// as interleaved RGB.
    pub(crate) fn from_base(base: &[u16], width: u32, height: u32, id: TileId) -> Self {
        let mut tile = Self::blank();
        let (x0, w, rows) = extent(width, height, id);
        for (ly, gy) in rows.enumerate() {
            let src = &base[3 * (gy * width as usize + x0)..][..3 * w];
            tile.rgb[3 * ly * TILE as usize..][..3 * w].copy_from_slice(src);
        }
        tile
    }

    /// Whether this tile's pixels inside the image are the original's.
    pub(crate) fn matches_base(&self, base: &[u16], width: u32, height: u32, id: TileId) -> bool {
        let (x0, w, rows) = extent(width, height, id);
        rows.enumerate().all(|(ly, gy)| {
            self.rgb[3 * ly * TILE as usize..][..3 * w]
                == base[3 * (gy * width as usize + x0)..][..3 * w]
        })
    }

    /// Bytes held, for memory budgets.
    pub(crate) const BYTES: usize = 8 * TILE_PIXELS;
}

/// The image pixels tile `id` covers.
pub(crate) fn rect(width: u32, height: u32, id: TileId) -> PixelRect {
    let (x0, w, rows) = extent(width, height, id);
    let (y0, h) = (rows.start, rows.len());
    PixelRect { x: x0 as u32, y: y0 as u32, width: w as u32, height: h as u32 }
}

/// Tile `id`'s first image column, its width inside the image, and the
/// image rows it covers.
pub(crate) fn extent(
    width: u32,
    height: u32,
    id: TileId,
) -> (usize, usize, std::ops::Range<usize>) {
    let (x0, y0) = (id.x * TILE, id.y * TILE);
    let w = (width - x0).min(TILE);
    let h = (height - y0).min(TILE);
    (x0 as usize, w as usize, y0 as usize..(y0 + h) as usize)
}
