//! The render function: a region of a source drawn through one look's
//! constants, tile by tile, optionally followed by a stage that reads each
//! pixel's neighbors. A region draws as that region of the whole picture.

use std::sync::Arc;

use deptheos_gpu::gpu_utils::{
    WORKGROUP_SIZE, create_bind_group, dispatch_compute_with, download_texture_with, tex,
};
use deptheos_gpu::{GpuContext, PooledTexture};

use super::{DngRenderPipeline, Planes, Prepared, PreparedRender, place};
use crate::local_tone::{self, Patch, Plane, Rect};

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

    fn rect(self) -> Rect {
        let n = |v: u32| v as usize;
        Rect { x: n(self.x), y: n(self.y), width: n(self.width), height: n(self.height) }
    }

    fn of(rect: Rect) -> Self {
        let n = |v: usize| u32::try_from(v).expect("a frame's pixels");
        Self { x: n(rect.x), y: n(rect.y), width: n(rect.width), height: n(rect.height) }
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

    /// The strips of whole rows of tiles of `side` covering this region.
    fn strips(self, side: u32) -> impl Iterator<Item = Region> {
        let y1 = self.end()[1];
        (self.y..y1).step_by(side as usize).map(move |y| Region {
            y,
            height: side.min(y1 - y),
            ..self
        })
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

/// The local tone's planes of one source kept between draws
/// ([`DngRenderPipeline::with_kept_planes`]), with what they were made
/// from.
#[derive(Default)]
pub struct KeptPlanes(Option<(super::PlanesKey, Arc<local_tone::Planes>)>);

impl KeptPlanes {
    /// Whether it holds planes.
    pub fn is_kept(&self) -> bool {
        self.0.is_some()
    }

    /// Bytes the kept planes hold.
    pub fn bytes(&self) -> u64 {
        self.0.as_ref().map_or(0, |(_, planes)| planes.bytes() as u64)
    }

    /// What the kept planes were made from.
    pub fn key(&self) -> Option<&super::PlanesKey> {
        self.0.as_ref().map(|(key, _)| key)
    }
}

/// The local tone's values of `region`, uploaded for a draw to read; a
/// placeholder texel without local tone.
fn upload_values(ctx: &GpuContext, region: Region, values: Option<Vec<[f32; 4]>>) -> PooledTexture {
    let (width, height, values) = match values {
        None => (1, 1, vec![[0.0f32; 4]]),
        Some(values) => (region.width, region.height, values),
    };
    let texture = ctx.pooled_texture(
        width,
        height,
        wgpu::TextureFormat::Rgba32Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        "local_planes",
    );
    ctx.queue.write_texture(
        texture.as_image_copy(),
        bytemuck::cast_slice(&values),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 16),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    texture
}

/// The local tone's values of a whole frame, each texel of a smaller
/// `size` grid the mean of the frame's values it covers, edge pixels
/// weighted by how much of them it covers, as the reduced copy is made: what
/// a reduced draw reads, so it takes the planes of the full-size pixels
/// (ADR-138).
pub struct GridPlanes {
    frame: [u32; 2],
    size: [u32; 2],
    /// Each frame column's (or row's) share of each grid column it falls in.
    across: Vec<Vec<(usize, f32)>>,
    down: Vec<Vec<(usize, f32)>>,
    sums: Vec<[f32; 4]>,
}

impl GridPlanes {
    /// An empty grid of `size` for a `frame`-sized picture.
    pub fn new(frame: [u32; 2], size: [u32; 2]) -> Self {
        let n = size[0] as usize * size[1] as usize;
        Self {
            frame,
            size,
            across: shares(frame[0], size[0]),
            down: shares(frame[1], size[1]),
            sums: vec![[0.0; 4]; n],
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Frame `region`'s values, row by row, added in.
    fn add(&mut self, region: Region, values: &[[f32; 4]]) {
        use rayon::prelude::*;
        let (gw, w) = (self.size[0] as usize, region.width as usize);
        let across = &self.across[region.x as usize..][..w];
        // Each row reduced across first, then added to the grid rows it
        // falls in.
        let rows: Vec<Vec<[f32; 4]>> = values
            .par_chunks(w)
            .map(|row| {
                let mut out = vec![[0.0f32; 4]; gw];
                for (v, shares) in row.iter().zip(across) {
                    for &(gx, share) in shares {
                        for c in 0..4 {
                            out[gx][c] += v[c] * share;
                        }
                    }
                }
                out
            })
            .collect();
        for (dy, row) in rows.iter().enumerate() {
            for &(gy, share) in &self.down[region.y as usize + dy] {
                let line = &mut self.sums[gy * gw..][..gw];
                for (o, v) in line.iter_mut().zip(row) {
                    for c in 0..4 {
                        o[c] += v[c] * share;
                    }
                }
            }
        }
    }

    /// The means, as a texture [`DngRenderPipeline::draw_reading`] takes.
    pub fn texture(&self, ctx: &GpuContext) -> wgpu::Texture {
        let area = (self.frame[0] as f32 / self.size[0] as f32)
            * (self.frame[1] as f32 / self.size[1] as f32);
        let means: Vec<[f32; 4]> = self.sums.iter().map(|s| s.map(|v| v / area)).collect();
        let [width, height] = self.size;
        let texture = ctx.create_texture(
            width,
            height,
            wgpu::TextureFormat::Rgba32Float,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            "grid_planes",
        );
        ctx.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&means),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 16),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        texture
    }
}

/// For each of `from` pixels, the grid cells of `to` it falls in and how
/// much of it falls in each, in pixels.
fn shares(from: u32, to: u32) -> Vec<Vec<(usize, f32)>> {
    let step = f64::from(from) / f64::from(to);
    (0..from)
        .map(|i| {
            let (a, b) = (f64::from(i), f64::from(i) + 1.0);
            let first = (a / step).floor() as usize;
            let last = (((b / step).ceil() as usize).max(first + 1)).min(to as usize);
            (first..last)
                .filter_map(|g| {
                    let (lo, hi) = (g as f64 * step, (g + 1) as f64 * step);
                    let share = b.min(hi) - a.max(lo);
                    (share > 0.0).then_some((g, share as f32))
                })
                .collect()
        })
        .collect()
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

    /// The log brightness the local tone's planes are taken of, over frame
    /// `region` of `source`.
    fn log_brightness(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
    ) -> Plane {
        assert!(source.covers.contains(region), "{region:?} outside {:?}", source.covers);
        let _drawing = ctx.drawing();
        let output = ctx.pooled_texture(
            region.width,
            region.height,
            wgpu::TextureFormat::R32Float,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            "log_brightness",
        );
        let (in_view, out_view) = (
            source.texture.create_view(&Default::default()),
            output.create_view(&Default::default()),
        );
        for tile in region.tiles(TILE) {
            let [sx, sy] = source.texel(tile.x, tile.y);
            let placement = place::Placement {
                origins: glam::UVec4::new(sx, sy, tile.x - region.x, tile.y - region.y),
                size: glam::UVec4::new(tile.width, tile.height, 0, 0),
                frame: glam::UVec4::new(tile.x, tile.y, source.frame[0], source.frame[1]),
            };
            let placement = ctx.create_uniform_buffer(&placement, "log_brightness_place");
            let (shader, entries): (_, Vec<wgpu::BindingResource>) = match &prepared.buffers {
                Prepared::Linear { params, hsm, look, .. } => (
                    &self.linear,
                    vec![
                        tex(&in_view),
                        params.as_entire_binding(),
                        hsm.as_entire_binding(),
                        look.as_entire_binding(),
                        placement.as_entire_binding(),
                        tex(&out_view),
                    ],
                ),
                Prepared::Standard { params, decode, .. } => (
                    &self.standard,
                    vec![
                        tex(&in_view),
                        params.as_entire_binding(),
                        decode.as_entire_binding(),
                        placement.as_entire_binding(),
                        tex(&out_view),
                    ],
                ),
            };
            let bindings: &[u32] = match &prepared.buffers {
                Prepared::Linear { .. } => &[0, 2, 4, 5, 6, 9],
                Prepared::Standard { .. } => &[0, 2, 3, 4, 7],
            };
            let layout = shader.brightness.get_bind_group_layout(0);
            let entries: Vec<wgpu::BindGroupEntry> = bindings
                .iter()
                .zip(entries)
                .map(|(&binding, resource)| wgpu::BindGroupEntry { binding, resource })
                .collect();
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("log_brightness"),
                layout: &layout,
                entries: &entries,
            });
            let mut encoder = ctx.encoder("log_brightness");
            dispatch_compute_with(
                &mut encoder,
                &shader.brightness,
                &[&group],
                "log_brightness",
                tile.width,
                tile.height,
                WORKGROUP_SIZE,
            );
            ctx.submit(encoder);
        }
        let mut data = Vec::with_capacity(region.width as usize * region.height as usize);
        download_texture_with(ctx, &output, [0, 0], [region.width, region.height], |row| {
            data.extend(row.as_chunks::<4>().0.iter().map(|b| f32::from_ne_bytes(*b)));
        })
        .expect("read back log brightness");
        Plane { width: region.width as usize, height: region.height as usize, data }
    }

    /// `prepared` with the local tone's planes of `source`'s whole frame,
    /// read `band` rows at a time. Without local tone it is returned as it
    /// is.
    pub fn with_planes(
        &self,
        ctx: &GpuContext,
        mut prepared: PreparedRender,
        source: Source<'_>,
        band: u32,
    ) -> PreparedRender {
        let Planes::Needed(needs) = prepared.planes else { return prepared };
        let frame = [source.frame[0] as usize, source.frame[1] as usize];
        let planes = local_tone::Planes::new(frame, band as usize, needs, |rect| {
            self.log_brightness(ctx, &prepared, source, Region::of(rect))
        });
        tracing::debug!("local tone planes: {needs:?} built");
        prepared.planes = Planes::Built(Arc::new(planes));
        prepared
    }

    /// [`Self::with_planes`], taking them from `kept` when it holds the
    /// planes these constants read of this `source`, and keeping them
    /// there. The caller empties `kept` when the source's pixels change.
    pub fn with_kept_planes(
        &self,
        ctx: &GpuContext,
        constants: &super::Constants,
        source: Source<'_>,
        band: u32,
        kept: &mut KeptPlanes,
    ) -> PreparedRender {
        let mut prepared = PreparedRender::new(ctx, constants);
        let Some(key) = constants.planes_key(source.frame) else { return prepared };
        if let Some((held, planes)) = &kept.0
            && *held == key
        {
            prepared.planes = Planes::Built(planes.clone());
            return prepared;
        }
        let prepared = self.with_planes(ctx, prepared, source, band);
        if let Planes::Built(planes) = &prepared.planes {
            kept.0 = Some((key, planes.clone()));
        }
        prepared
    }

    /// `prepared`'s constants with the planes `kept` holds when they are
    /// of the same sliders (`PlanesKey::same_sliders`), though made under
    /// another white or profile: a drag's draw, which doesn't wait for new
    /// planes. `None` when it holds none such.
    pub fn with_planes_like(
        &self,
        ctx: &GpuContext,
        constants: &super::Constants,
        frame: [u32; 2],
        kept: &KeptPlanes,
    ) -> Option<PreparedRender> {
        let key = constants.planes_key(frame)?;
        let (held, planes) = kept.0.as_ref()?;
        held.same_sliders(&key).then(|| {
            let mut prepared = PreparedRender::new(ctx, constants);
            prepared.planes = Planes::Built(planes.clone());
            prepared
        })
    }

    /// The local tone's values over frame `region`
    /// ([`local_tone::Planes::at`]), row by row; `None` without local tone.
    fn region_values(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
    ) -> Option<Vec<[f32; 4]>> {
        match &prepared.planes {
            Planes::None => None,
            Planes::Needed(_) => panic!("local tone draws with its planes (`with_planes`)"),
            Planes::Built(planes) => {
                let around = planes.around(region.rect());
                let plane = self.log_brightness(ctx, prepared, source, Region::of(around));
                Some(planes.at(&Patch { rect: around, plane }, region.rect()))
            }
        }
    }

    /// The local tone's values over frame `region` as a texture a draw
    /// reads at output texels, its texel (0, 0) the region's top-left; a
    /// placeholder without local tone. [`Self::draw_reading`] takes it.
    pub fn region_planes(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
    ) -> PooledTexture {
        let values = self.region_values(ctx, prepared, source, region);
        upload_values(ctx, region, values)
    }

    /// The local tone's values of `source`'s whole frame gathered into
    /// `grid`, a strip of [`TILE`] rows at a time.
    pub fn gather_planes(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        grid: &mut GridPlanes,
    ) {
        let whole = Region::whole(source.frame[0], source.frame[1]);
        for strip in whole.strips(TILE) {
            if let Some(values) = self.region_values(ctx, prepared, source, strip) {
                grid.add(strip, &values);
            }
        }
    }

    /// [`Self::draw`] over the whole frame, its local tone's values also
    /// gathered into `grid`, so a reduced draw reads the planes this draw
    /// did.
    pub fn draw_gathering(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        output: &wgpu::Texture,
        grid: &mut GridPlanes,
    ) {
        let whole = Region::whole(source.frame[0], source.frame[1]);
        self.draw_strips(ctx, prepared, source, whole, output, TILE, Some(grid));
    }

    /// Frame `region` of `source` through `prepared` into `output`'s
    /// top-left, reading the local tone's values from `planes`, whose
    /// texel (0, 0) holds the region's top-left ([`Self::region_planes`],
    /// or a reduced copy's [`GridPlanes::texture`]), rather than making
    /// them.
    pub fn draw_reading(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        planes: &wgpu::Texture,
        output: &wgpu::Texture,
    ) {
        assert!(source.covers.contains(region), "{region:?} outside {:?}", source.covers);
        assert!(region.width <= output.width() && region.height <= output.height());
        let _drawing = ctx.drawing();
        let views = self.views(source, output);
        let planes_view = planes.create_view(&Default::default());
        for strip in region.strips(TILE) {
            let at = [region.x, region.y];
            self.draw_strip_tiles(
                ctx,
                prepared,
                source,
                region,
                strip,
                TILE,
                &views,
                &planes_view,
                at,
            );
        }
    }

    fn views(&self, source: Source<'_>, output: &wgpu::Texture) -> [wgpu::TextureView; 2] {
        [source.texture.create_view(&Default::default()), output.create_view(&Default::default())]
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
        self.draw_strips(ctx, prepared, source, region, output, side, None);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_strips(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        output: &wgpu::Texture,
        side: u32,
        mut grid: Option<&mut GridPlanes>,
    ) {
        assert!(source.covers.contains(region), "{region:?} outside {:?}", source.covers);
        assert!(region.width <= output.width() && region.height <= output.height());
        let _drawing = ctx.drawing();
        let views = self.views(source, output);
        // The local tone's planes are made a strip of tiles at a time, so a
        // whole picture never holds them at once.
        for strip in region.strips(side) {
            let values = self.region_values(ctx, prepared, source, strip);
            if let (Some(grid), Some(values)) = (grid.as_deref_mut(), &values) {
                grid.add(strip, values);
            }
            let planes = upload_values(ctx, strip, values);
            let planes_view = planes.create_view(&Default::default());
            let at = [strip.x, strip.y];
            self.draw_strip_tiles(
                ctx,
                prepared,
                source,
                region,
                strip,
                side,
                &views,
                &planes_view,
                at,
            );
        }
    }

    /// The tiles of `side` of `strip`, frame pixel `planes_at` at the
    /// planes' texel (0, 0).
    #[allow(clippy::too_many_arguments)]
    fn draw_strip_tiles(
        &self,
        ctx: &GpuContext,
        prepared: &PreparedRender,
        source: Source<'_>,
        region: Region,
        strip: Region,
        side: u32,
        views: &[wgpu::TextureView; 2],
        planes_view: &wgpu::TextureView,
        planes_at: [u32; 2],
    ) {
        let [in_view, out_view] = views;
        for tile in strip.tiles(side) {
            let [sx, sy] = source.texel(tile.x, tile.y);
            let placement = place::Placement {
                origins: glam::UVec4::new(sx, sy, tile.x - region.x, tile.y - region.y),
                size: glam::UVec4::new(
                    tile.width,
                    tile.height,
                    tile.x - planes_at[0],
                    tile.y - planes_at[1],
                ),
                frame: glam::UVec4::new(tile.x, tile.y, source.frame[0], source.frame[1]),
            };
            let placement = ctx.create_uniform_buffer(&placement, "dng_render_place");
            let (shader, entries): (_, Vec<wgpu::BindingResource>) = match &prepared.buffers {
                Prepared::Linear { params, tone, hsm, look, curve } => (
                    &self.linear,
                    vec![
                        tex(in_view),                  // in_tex
                        tex(out_view),                 // out_tex
                        params.as_entire_binding(),    // params
                        tone.as_entire_binding(),      // tone_curve
                        hsm.as_entire_binding(),       // hsm_lut
                        look.as_entire_binding(),      // look_lut
                        placement.as_entire_binding(), // place
                        curve.as_entire_binding(),     // curve_luts
                        tex(planes_view),              // local_planes
                    ],
                ),
                Prepared::Standard { params, decode, curve } => (
                    &self.standard,
                    vec![
                        tex(in_view),                  // in_tex
                        tex(out_view),                 // out_tex
                        params.as_entire_binding(),    // params
                        decode.as_entire_binding(),    // decode
                        placement.as_entire_binding(), // place
                        curve.as_entire_binding(),     // curve_luts
                        tex(planes_view),              // local_planes
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
        let _drawing = ctx.drawing();
        let grown = region.grown(stage.margin, source.covers);
        let around = Self::pooled_output(ctx, grown.width, grown.height);
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

    /// With every slider that reads neighbors, a draw in small tiles, whose
    /// planes are made strip by strip, equals one in a single tile.
    #[test]
    fn strips_of_planes_draw_as_one() {
        use deptheos_core::upload::ImageUploader;
        let Some(ctx) = GpuContext::new_or_skip() else { return };
        let (w, h) = (230u32, 170u32);
        let pixels: Vec<u16> = (0..w * h * 3)
            .map(|i| {
                let (x, y) = (i / 3 % w, i / 3 / w);
                let block = if (x / 40 + y / 30) % 2 == 0 { 4000 } else { 45_000 };
                (block + (i.wrapping_mul(2_654_435_761) >> 21) % 5000) as u16
            })
            .collect();
        let mut uploader = ImageUploader::new(&ctx);
        let prep = uploader.prepare_rgb16(&ctx, &pixels, w, h).unwrap();
        let input = ImageUploader::create_target(&ctx, w, h, "strips_source");
        let mut encoder = ctx.encoder("strips_upload");
        prep.dispatch_to_rgba(&ctx, &mut encoder, &input);
        ctx.submit(encoder);
        let adjustments = crate::Adjustments {
            highlights: -40.0,
            shadows: 50.0,
            clarity: 30.0,
            texture: 40.0,
            sharpening: crate::Sharpening { amount: 60.0, ..crate::Sharpening::NONE },
            ..crate::Adjustments::NEUTRAL
        };
        let color = crate::Color::Standard(None);
        let constants = crate::resolve_adjusted(color, &adjustments, crate::OutputSpace::Srgb);
        let pipeline = DngRenderPipeline::new(&ctx);
        let source = Source::whole(&input);
        let prepared = pipeline.with_planes(&ctx, PreparedRender::new(&ctx, &constants), source, h);
        let whole = Region::whole(w, h);
        let drawn = |side| {
            let output = DngRenderPipeline::create_output(&ctx, w, h);
            pipeline.draw_tiled(&ctx, &prepared, source, whole, &output, side);
            let mut bytes = Vec::new();
            download_texture_with(&ctx, &output, [0, 0], [w, h], |row| {
                bytes.extend_from_slice(row)
            })
            .unwrap();
            bytes
        };
        assert!(drawn(TILE) == drawn(48));
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
