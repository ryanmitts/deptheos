//! The painted canvas as display tiles: touched full-size tiles
//! re-rendered, the change halved into the smaller levels on the CPU.

use std::collections::BTreeSet;
use std::sync::Arc;

use deptheos_gpu::GpuContext;
use deptheos_render::dng_render::DngRenderPipeline;

use super::canvas::Canvas;
use super::render::{RectPixels, RectRenderer};
use super::tile::{self, TileId};
use crate::Error;
use crate::engine::look::Look;
use crate::engine::present::{
    CpuImage, PixelRect, PresentedImage, TILE_CORE, core_rect, cut_tile, present_rect, stored_rect,
    tiles_touching,
};

/// One display tile's new pixels, gutter included, in the
/// [`PresentedImage`]'s grid (unrelated to canvas tiles).
pub struct DisplayPatch {
    pub level: usize,
    pub tx: u32,
    pub ty: u32,
    pub tile: CpuImage,
}

/// The display of the canvas being painted.
pub(crate) struct CanvasDisplay {
    /// Size of every level, full size first.
    sizes: Vec<[u32; 2]>,
    /// Levels 1.. as whole images (level 0 is rendered from the canvas).
    levels: Vec<CpuImage>,
    renderer: RectRenderer,
}

impl CanvasDisplay {
    /// From the canvas's whole display, which must be full size first.
    pub(crate) fn new(ctx: &GpuContext, display: &PresentedImage) -> Self {
        assert_eq!(display.first_level, 0, "a canvas display starts at full size");
        let sizes = display.levels.iter().map(|l| [l.width, l.height]).collect();
        let levels = display.levels.iter().skip(1).map(|l| l.assemble()).collect();
        Self { sizes, levels, renderer: RectRenderer::new(ctx) }
    }

    /// The display tiles `dirty` changed, at every level. Each full-size
    /// tile renders on its own, so a diagonal stroke doesn't render its
    /// bounding box.
    pub(crate) fn update(
        &mut self,
        ctx: &Arc<GpuContext>,
        color: &DngRenderPipeline,
        canvas: &Canvas,
        dirty: &BTreeSet<TileId>,
    ) -> Result<Vec<DisplayPatch>, Error> {
        let full = self.sizes[0];
        let changed: Vec<PixelRect> =
            dirty.iter().map(|&id| tile::rect(full[0], full[1], id)).collect();
        let mut patches = Vec::new();
        // What changed of the next level down, as rects of its pixels.
        let mut halves = Vec::new();
        for [tx, ty] in touching(full, &changed) {
            // The tile's stored pixels, and its core grown to what the
            // smaller levels are halved from.
            let core = core_rect(full, tx, ty);
            let rect = stored_rect(full, tx, ty).union(core.aligned(full[0], full[1]));
            let patch = self.render_rect(ctx, color, canvas, rect)?;
            let tile = cut_tile(&patch, [rect.x, rect.y], full, tx, ty);
            patches.push(DisplayPatch { level: 0, tx, ty, tile });
            // A tile whose gutter alone changed has nothing to halve.
            let inside = changed.iter().filter_map(|r| r.intersect(core)).reduce(PixelRect::union);
            if let (Some(inside), Some(level)) = (inside, self.levels.first_mut()) {
                let half = inside.aligned(full[0], full[1]).halved();
                present_rect(&patch, [rect.x, rect.y], full, level, half);
                halves.push(half);
            }
        }
        for k in 1..self.sizes.len() {
            if k > 1 {
                let (done, rest) = self.levels.split_at_mut(k - 1);
                for half in &mut halves {
                    *half = half.halved();
                    present_rect(&done[k - 2], [0, 0], self.sizes[k - 1], &mut rest[0], *half);
                }
            }
            for [tx, ty] in touching(self.sizes[k], &halves) {
                let tile = cut_tile(&self.levels[k - 1], [0, 0], self.sizes[k], tx, ty);
                patches.push(DisplayPatch { level: k, tx, ty, tile });
            }
        }
        Ok(patches)
    }

    /// Full-size display tiles `[tx, ty]`, for a shell that keeps only
    /// some of level 0; tiles past the level are skipped.
    pub(crate) fn render_tiles(
        &mut self,
        ctx: &Arc<GpuContext>,
        color: &DngRenderPipeline,
        canvas: &Canvas,
        tiles: &[[u32; 2]],
    ) -> Result<Vec<DisplayPatch>, Error> {
        let full = self.sizes[0];
        let [cols, rows] = [full[0].div_ceil(TILE_CORE), full[1].div_ceil(TILE_CORE)];
        let mut patches = Vec::with_capacity(tiles.len());
        for &[tx, ty] in tiles.iter().filter(|[tx, ty]| *tx < cols && *ty < rows) {
            let rect = stored_rect(full, tx, ty);
            let patch = self.render_rect(ctx, color, canvas, rect)?;
            let tile = cut_tile(&patch, [rect.x, rect.y], full, tx, ty);
            patches.push(DisplayPatch { level: 0, tx, ty, tile });
        }
        Ok(patches)
    }

    /// The canvas's pixels of `rect` (full size), rendered for display.
    fn render_rect(
        &mut self,
        ctx: &Arc<GpuContext>,
        color: &DngRenderPipeline,
        canvas: &Canvas,
        rect: PixelRect,
    ) -> Result<CpuImage, Error> {
        let pixels = canvas.read(rect);
        let stack = canvas.stack();
        let input = RectPixels { pixels: &pixels, width: rect.width, height: rect.height };
        // Retouch shows the stored picture.
        self.renderer.render(ctx, color, input, Look::stored(stack))
    }
}

/// The display tiles of a `level`-sized level whose stored pixels (core
/// and gutter) include any of `rects`, each once.
fn touching(level: [u32; 2], rects: &[PixelRect]) -> BTreeSet<[u32; 2]> {
    let mut tiles = BTreeSet::new();
    for &rect in rects {
        let [cols, rows] = tiles_touching(level, rect);
        tiles.extend(rows.flat_map(|ty| cols.clone().map(move |tx| [tx, ty])));
    }
    tiles
}
