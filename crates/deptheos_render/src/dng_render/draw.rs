//! The render function: a region of a source drawn through one look's
//! constants, tile by tile, optionally followed by a stage that reads each
//! pixel's neighbors. A region draws as that region of the whole picture.

use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::{WORKGROUP_SIZE, create_bind_group, dispatch_compute_with, tex};

use super::{DngRenderPipeline, Prepared, PreparedRender, place};

/// The side of a tile, in pixels: one dispatch and one submit each, so no
/// single GPU command runs long on a large picture.
pub const TILE: u32 = 2048;

/// A rectangle of the frame, in its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// All of a `width`×`height` frame.
    pub fn whole(width: u32, height: u32) -> Self {
        Self { x: 0, y: 0, width, height }
    }

    fn end(self) -> [u32; 2] {
        [self.x + self.width, self.y + self.height]
    }

    fn contains(self, inner: Region) -> bool {
        let ([x1, y1], [ix1, iy1]) = (self.end(), inner.end());
        inner.x >= self.x && inner.y >= self.y && ix1 <= x1 && iy1 <= y1
    }

    /// Grown by `margin` on every side, kept within `bounds`.
    fn grown(self, margin: u32, bounds: Region) -> Self {
        let [x1, y1] = self.end();
        let [bx1, by1] = bounds.end();
        let x = self.x.saturating_sub(margin).max(bounds.x);
        let y = self.y.saturating_sub(margin).max(bounds.y);
        let (x_end, y_end) = ((x1 + margin).min(bx1), (y1 + margin).min(by1));
        Self { x, y, width: x_end - x, height: y_end - y }
    }

    /// The tiles of `side` covering this region, row by row.
    fn tiles(self, side: u32) -> impl Iterator<Item = Region> {
        let [x1, y1] = self.end();
        (self.y..y1).step_by(side as usize).flat_map(move |y| {
            (self.x..x1).step_by(side as usize).map(move |x| Region {
                x,
                y,
                width: side.min(x1 - x),
                height: side.min(y1 - y),
            })
        })
    }
}

/// Pixels to draw from: a texture holding `covers` of a `frame`-sized
/// picture, its texel (0, 0) at the region's top-left. A full-size source
/// covers the whole frame; a rectangle of paint's canvas covers itself.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    texture: &'a wgpu::Texture,
    covers: Region,
    frame: [u32; 2],
}

impl<'a> Source<'a> {
    /// A texture of the whole frame.
    pub fn whole(texture: &'a wgpu::Texture) -> Self {
        let (width, height) = (texture.width(), texture.height());
        Self { texture, covers: Region::whole(width, height), frame: [width, height] }
    }

    /// The top-left of `texture`, holding `covers` of a `frame`-sized
    /// picture.
    pub fn part(texture: &'a wgpu::Texture, covers: Region, frame: [u32; 2]) -> Self {
        assert!(
            covers.width <= texture.width() && covers.height <= texture.height(),
            "a {}x{} part in a {}x{} texture",
            covers.width,
            covers.height,
            texture.width(),
            texture.height()
        );
        assert!(Region::whole(frame[0], frame[1]).contains(covers), "{covers:?} outside {frame:?}");
        Self { texture, covers, frame }
    }

    /// The source texel holding frame pixel (`x`, `y`).
    fn texel(&self, x: u32, y: u32) -> [u32; 2] {
        [x - self.covers.x, y - self.covers.y]
    }
}

/// A stage that reads each pixel's neighbors, run after the per-pixel
/// pass: the pass draws the region grown by `margin` (within what the
/// source covers), and the stage writes the region from it.
///
/// Its WGSL entry point `stage` binds, in group 0: the per-pixel result
/// around the region (`texture_2d<f32>`), the output (a write-only
/// storage texture of `ctx.source_format()`, `SOURCE_RGBA16UNORM` defined
/// when it is Rgba16Unorm), a `NeighborPlace` uniform (`neighbor_place`
/// below), and a whole-frame
/// input (`texture_2d<f32>`) read at frame coordinates. It may read up to
/// `margin` pixels from each it writes, clamped to the input's rectangle.
pub struct NeighborStage {
    pipeline: wgpu::ComputePipeline,
    margin: u32,
}

impl NeighborStage {
    pub fn new(ctx: &GpuContext, wgsl: &str, label: &str, margin: u32) -> Self {
        let module = ctx.create_shader_module_composed(wgsl, label, &[]);
        let pipeline =
            deptheos_gpu::gpu_utils::create_compute_pipeline(&ctx.device, &module, "stage");
        Self { pipeline, margin }
    }
}

mod neighbor_place {
    #![allow(dead_code)] // encase derive emits layout-check fns flagged as unused

    use glam::UVec4;

    /// Mirror of a neighbor stage's `NeighborPlace` uniform, all in frame
    /// pixels: its tile (x, y, width, height), the per-pixel result's
    /// rectangle, and the output texel the tile's first pixel goes to (xy)
    /// with the frame's size (zw).
    #[derive(Clone, Copy, Debug, encase::ShaderType)]
    pub(super) struct NeighborPlace {
        pub(super) tile: UVec4,
        pub(super) input: UVec4,
        pub(super) out_frame: UVec4,
    }
}
use neighbor_place::NeighborPlace;

impl DngRenderPipeline {
    /// Draw frame `region` of `source` through `prepared` into `output`'s
    /// top-left, in tiles of [`TILE`].
    pub fn draw(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        output: &wgpu::Texture,
    ) {
        self.draw_tiled(ctx, prepared, source, region, output, TILE);
    }

    /// [`Self::draw`], then `stage` over the region, reading
    /// `frame_input` at frame coordinates.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        stage: &NeighborStage,
        frame_input: &wgpu::Texture,
        output: &wgpu::Texture,
    ) {
        self.draw_with_tiled(ctx, prepared, source, region, stage, frame_input, output, TILE);
    }

    pub(super) fn draw_tiled(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        output: &wgpu::Texture,
        side: u32,
    ) {
        assert!(source.covers.contains(region), "{region:?} outside {:?}", source.covers);
        assert!(region.width <= output.width() && region.height <= output.height());
        let (in_view, out_view) = (
            source.texture.create_view(&Default::default()),
            output.create_view(&Default::default()),
        );
        for tile in region.tiles(side) {
            let [sx, sy] = source.texel(tile.x, tile.y);
            let placement = place::Placement {
                origins: glam::UVec4::new(sx, sy, tile.x - region.x, tile.y - region.y),
                size: glam::UVec4::new(tile.width, tile.height, 0, 0),
            };
            let placement = ctx.create_uniform_buffer(&placement, "dng_render_place");
            let (shader, entries): (_, Vec<wgpu::BindingResource>) = match &prepared.0 {
                Prepared::Linear { params, tone, hsm, look } => (
                    &self.linear,
                    vec![
                        tex(&in_view),                 // in_tex
                        tex(&out_view),                // out_tex
                        params.as_entire_binding(),    // params
                        tone.as_entire_binding(),      // tone_curve
                        hsm.as_entire_binding(),       // hsm_lut
                        look.as_entire_binding(),      // look_lut
                        placement.as_entire_binding(), // place
                    ],
                ),
                Prepared::Standard { params, decode } => (
                    &self.standard,
                    vec![
                        tex(&in_view),                 // in_tex
                        tex(&out_view),                // out_tex
                        params.as_entire_binding(),    // params
                        decode.as_entire_binding(),    // decode
                        placement.as_entire_binding(), // place
                    ],
                ),
            };
            let group0 =
                create_bind_group(&ctx.device, &shader.pipeline, 0, "dng_render", &entries);
            let groups: Vec<&wgpu::BindGroup> =
                std::iter::once(&group0).chain(shader.thresholds.as_ref()).collect();
            let mut encoder = ctx.encoder("dng_render");
            dispatch_compute_with(
                &mut encoder,
                &shader.pipeline,
                &groups,
                "dng_render",
                tile.width,
                tile.height,
                WORKGROUP_SIZE,
            );
            ctx.submit(encoder);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_with_tiled(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        stage: &NeighborStage,
        frame_input: &wgpu::Texture,
        output: &wgpu::Texture,
        side: u32,
    ) {
        let grown = region.grown(stage.margin, source.covers);
        let around = Self::create_output(ctx, grown.width, grown.height);
        self.draw_tiled(ctx, prepared, source, grown, &around, side);

        let views =
            [around.create_view(&Default::default()), output.create_view(&Default::default())];
        let frame_view = frame_input.create_view(&Default::default());
        for tile in region.tiles(side) {
            let placement = NeighborPlace {
                tile: glam::UVec4::new(tile.x, tile.y, tile.width, tile.height),
                input: glam::UVec4::new(grown.x, grown.y, grown.width, grown.height),
                out_frame: glam::UVec4::new(
                    tile.x - region.x,
                    tile.y - region.y,
                    source.frame[0],
                    source.frame[1],
                ),
            };
            let placement = ctx.create_uniform_buffer(&placement, "neighbor_place");
            let group = create_bind_group(
                &ctx.device,
                &stage.pipeline,
                0,
                "neighbor_stage",
                &[tex(&views[0]), tex(&views[1]), placement.as_entire_binding(), tex(&frame_view)],
            );
            let mut encoder = ctx.encoder("neighbor_stage");
            dispatch_compute_with(
                &mut encoder,
                &stage.pipeline,
                &[&group],
                "neighbor_stage",
                tile.width,
                tile.height,
                WORKGROUP_SIZE,
            );
            ctx.submit(encoder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_a_region_once() {
        let region = Region { x: 5, y: 7, width: 130, height: 65 };
        let tiles: Vec<_> = region.tiles(64).collect();
        assert_eq!(tiles.len(), 3 * 2);
        assert_eq!(tiles[2], Region { x: 133, y: 7, width: 2, height: 64 });
        assert_eq!(tiles[5], Region { x: 133, y: 71, width: 2, height: 1 });
        let area: u32 = tiles.iter().map(|t| t.width * t.height).sum();
        assert_eq!(area, 130 * 65);
    }

    #[test]
    fn a_margin_stays_in_bounds() {
        let bounds = Region::whole(100, 80);
        let inside = Region { x: 10, y: 20, width: 30, height: 30 };
        assert_eq!(inside.grown(4, bounds), Region { x: 6, y: 16, width: 38, height: 38 });
        let corner = Region { x: 1, y: 70, width: 99, height: 10 };
        assert_eq!(corner.grown(4, bounds), Region { x: 0, y: 66, width: 100, height: 14 });
    }
}
