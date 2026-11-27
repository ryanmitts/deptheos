//! What an open render keeps on the GPU while Adjust is open (ADR-114):
//! the full-size source, the reduced copy the drag preview draws from, and
//! the local tone's planes, all made for one generation of the stored
//! picture with paint ([`crate::engine::generation`]). Never saved; exports
//! and thumbnails draw from the stored pixels. The preview's two tiers
//! draw from them ([`GpuCopies::draw_preview`], ADR-138).

use deptheos_gpu::PooledTexture;
use deptheos_render::dng_render::{
    Constants, DngRenderPipeline, Encoding, GridPlanes, KeptPlanes, PlanesKey, PreparedRender,
    REDUCED_FORMAT, Region, Source, TILE, reduced_size,
};
use deptheos_render::{Adjustments, Draw, OutputSpace, Reframe};

use super::preview::{Draft, FlatPicture, FlatSlot, Shown, Tier, fit_size, read_flat};
use crate::engine::Engine;
use crate::engine::generation::Generation;
use crate::runs::session::CompletedStack;
use crate::{Error, Look, Result};

/// One open render's GPU copies.
pub struct GpuCopies {
    generation: Generation,
    /// The stored picture with paint, as the draw takes it.
    source: wgpu::Texture,
    /// At most 4096 on the long side, linear, in 16-bit float.
    reduced: wgpu::Texture,
    /// How the reduced copy and its fit copy take their means.
    encoding: Encoding,
    planes: KeptPlanes,
    /// The fit tier's: the reduced copy at the size shown.
    fit: Option<([u32; 2], wgpu::Texture)>,
    /// The local tone's values of the whole frame on a grid of the fit
    /// tier's largest size, gathered from the full-size pixels, and what
    /// their planes were made from.
    grid: Option<(PlanesKey, wgpu::Texture)>,
    /// The grid at the size shown.
    fit_planes: Option<(PlanesKey, [u32; 2], wgpu::Texture)>,
    /// The region tier's: the local tone's values of the region last
    /// drawn.
    region: Option<(Region, PlanesKey, PooledTexture)>,
}

impl GpuCopies {
    /// The copies of `stack` with `pixels` (its own, or with paint) at
    /// `generation`.
    pub fn make(
        engine: &Engine,
        stack: &CompletedStack,
        pixels: &[u16],
        generation: Generation,
    ) -> Result<Self> {
        let constants = source_constants(stack)?;
        let source = engine.upload_for(&constants, pixels, stack.width, stack.height)?;
        let encoding = engine.color().encoding(engine.gpu(), &constants);
        let reduced = engine.color().reduce(engine.gpu(), &source, &constants, &encoding);
        engine.gpu().check().map_err(deptheos_core::CoreError::from)?;
        let copies = Self {
            generation,
            source,
            reduced,
            encoding,
            planes: KeptPlanes::default(),
            fit: None,
            grid: None,
            fit_planes: None,
            region: None,
        };
        tracing::debug!("adjust: GPU copies made, {}", copies.memory());
        Ok(copies)
    }

    /// The generation they were made for: copies of another are stale.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The full-size source.
    pub fn source(&self) -> Source<'_> {
        Source::whole(&self.source)
    }

    /// The reduced copy, drawn with [`Constants::reduced`].
    pub fn reduced(&self) -> Source<'_> {
        Source::whole(&self.reduced)
    }

    /// The whole picture at full size through `constants`, the local
    /// tone's planes kept for the next draw and its values gathered for the
    /// fit tier, so a drag after it starts from this draw's planes.
    pub fn draw_full(&mut self, engine: &Engine, constants: &Constants) -> PooledTexture {
        let (color, ctx) = (engine.color(), engine.gpu());
        let frame = self.frame();
        // The region's values go first, so the redraw has their memory.
        self.region = None;
        let source = Source::whole(&self.source);
        let band = frame[1].min(TILE);
        let prepared = color.with_kept_planes(ctx, constants, source, band, &mut self.planes);
        let output = color_output(engine, frame[0], frame[1]);
        match constants.planes_key(frame) {
            Some(key) => {
                let mut grid = GridPlanes::new(frame, self.grid_size());
                color.draw_gathering(ctx, &prepared, source, &output, &mut grid);
                self.set_grid(key, grid.texture(ctx));
            }
            None => color.draw(ctx, &prepared, source, Region::whole(frame[0], frame[1]), &output),
        }
        output
    }

    /// One tick of the drag preview, or with [`Draft::Settled`] the
    /// picture it settles on, through `constants` (a look's for the
    /// screen, framed by the reframe in stored pixels), as one flat
    /// picture in a buffer from `slot`. At fit it is the whole picture at
    /// the size shown, from the reduced copy, without Texture and
    /// Sharpening; past [`super::preview::REGION_ZOOM`] the region on
    /// screen at full size.
    pub fn draw_preview(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        shown: Shown,
        draft: Draft,
        slot: &FlatSlot,
    ) -> Result<FlatPicture> {
        let ctx = engine.gpu();
        let frame = self.frame();
        let picture = match Tier::of(shown.zoom) {
            Tier::Fit => self.draw_fit(engine, constants, shown.zoom, draft, slot),
            Tier::Region => {
                let region = within(shown.visible, frame);
                self.draw_region(engine, constants, region, draft, slot)
            }
        };
        ctx.check().map_err(deptheos_core::CoreError::from)?;
        picture
    }

    fn draw_fit(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        zoom: f32,
        draft: Draft,
        slot: &FlatSlot,
    ) -> Result<FlatPicture> {
        let (color, ctx) = (engine.color(), engine.gpu());
        let [w, h] = self.frame();
        let size = fit_size(w, h, zoom, [self.reduced.width(), self.reduced.height()]);
        if self.fit.as_ref().is_none_or(|(s, _)| *s != size) {
            self.fit = Some((size, color.resample(ctx, &self.reduced, size, &self.encoding)));
        }
        let (sx, sy) = (size[0] as f32 / w as f32, size[1] as f32 / h as f32);
        let drawn = constants.clone().without_fine_detail().reduced().scaled(sx, sy);
        if drawn.has_local_tone() {
            self.fit_planes(engine, constants, size, draft);
        }
        let ((_, copy), planes) = (self.fit.as_ref().expect("made above"), &self.fit_planes);
        let prepared = PreparedRender::new(ctx, &drawn);
        let (output, whole) =
            (color_output(engine, size[0], size[1]), Region::whole(size[0], size[1]));
        match planes {
            Some((_, _, planes)) if drawn.has_local_tone() => {
                color.draw_reading(ctx, &prepared, Source::whole(copy), whole, planes, &output);
            }
            _ => color.draw(ctx, &prepared, Source::whole(copy), whole, &output),
        }
        read_flat(ctx, &output, size, Region::whole(w, h), constants.output(), slot)
    }

    fn draw_region(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        region: Region,
        draft: Draft,
        slot: &FlatSlot,
    ) -> Result<FlatPicture> {
        let (color, ctx) = (engine.color(), engine.gpu());
        let output = color_output(engine, region.width, region.height);
        let (prepared, made) = self.prepared(engine, constants, draft);
        let source = Source::whole(&self.source);
        match (made, constants.planes_key(self.frame())) {
            (Some(made), Some(key)) => {
                let kept = self.region.as_ref().is_some_and(|(r, k, _)| {
                    *r == region && (*k == key || draft == Draft::Tick && k.same_sliders(&key))
                });
                if !kept {
                    self.region = None;
                    let values = color.region_planes(ctx, &prepared, source, region);
                    self.region = Some((region, made, values));
                }
                let (_, _, values) = self.region.as_ref().expect("made above");
                color.draw_reading(ctx, &prepared, source, region, values, &output);
            }
            _ => color.draw(ctx, &prepared, source, region, &output),
        }
        let size = [region.width, region.height];
        read_flat(ctx, &output, size, region, constants.output(), slot)
    }

    /// `constants` prepared with the local tone's planes, and what those
    /// were made from: for a tick, planes kept from an earlier value of the
    /// same sliders; else made for exactly these.
    fn prepared(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        draft: Draft,
    ) -> (PreparedRender, Option<PlanesKey>) {
        let (color, ctx) = (engine.color(), engine.gpu());
        let frame = self.frame();
        if draft == Draft::Tick
            && let Some(prepared) = color.with_planes_like(ctx, constants, frame, &self.planes)
        {
            return (prepared, self.planes.key().cloned());
        }
        let source = Source::whole(&self.source);
        let band = frame[1].min(TILE);
        let prepared = color.with_kept_planes(ctx, constants, source, band, &mut self.planes);
        (prepared, self.planes.key().cloned().filter(|_| constants.has_local_tone()))
    }

    /// The local tone's values at the fit tier's `size`, from the grid,
    /// gathering it first unless it holds these planes (or, for a tick,
    /// planes of the same sliders).
    fn fit_planes(&mut self, engine: &Engine, constants: &Constants, size: [u32; 2], draft: Draft) {
        let (color, ctx) = (engine.color(), engine.gpu());
        let frame = self.frame();
        let Some(key) = constants.planes_key(frame) else { return };
        let fits =
            |held: &PlanesKey| *held == key || draft == Draft::Tick && held.same_sliders(&key);
        if !self.grid.as_ref().is_some_and(|(held, _)| fits(held)) {
            let (prepared, made) = self.prepared(engine, constants, draft);
            let mut grid = GridPlanes::new(frame, self.grid_size());
            color.gather_planes(ctx, &prepared, Source::whole(&self.source), &mut grid);
            self.set_grid(made.unwrap_or(key), grid.texture(ctx));
        }
        let (grid_key, grid) = self.grid.as_ref().expect("gathered above");
        if !self.fit_planes.as_ref().is_some_and(|(k, s, _)| k == grid_key && *s == size) {
            let resampled = color.resample_planes(ctx, grid, size);
            self.fit_planes = Some((grid_key.clone(), size, resampled));
        }
    }

    fn set_grid(&mut self, key: PlanesKey, grid: wgpu::Texture) {
        self.fit_planes = None;
        self.grid = Some((key, grid));
    }

    /// The grid the fit tier's planes are gathered on: its largest size.
    fn grid_size(&self) -> [u32; 2] {
        let [w, h] = self.frame();
        fit_size(w, h, 1.0, [self.reduced.width(), self.reduced.height()])
    }

    fn frame(&self) -> [u32; 2] {
        [self.source.width(), self.source.height()]
    }

    /// What they hold now and what a full redraw adds.
    pub fn memory(&self) -> RenderMemory {
        let (width, height) = (self.source.width(), self.source.height());
        let texel = |t: &wgpu::Texture| u64::from(t.format().block_copy_size(None).unwrap_or(16));
        RenderMemory {
            source: texel(&self.source) * pixels(width, height),
            reduced: texel(&self.reduced) * pixels(self.reduced.width(), self.reduced.height()),
            planes: self.planes.bytes(),
            fit: [self.fit.as_ref().map(|(_, t)| t), self.grid.as_ref().map(|(_, t)| t)]
                .into_iter()
                .chain([self.fit_planes.as_ref().map(|(_, _, t)| t)])
                .flatten()
                .map(|t| texel(t) * pixels(t.width(), t.height()))
                .sum(),
            ..RenderMemory::of(width, height, texel(&self.source))
        }
    }

    /// Whether a tick of `constants` drawn for `shown` reads the planes
    /// they make, so it is the settled picture there: else the session
    /// draws the full picture first, which makes them (ADR-138).
    pub fn is_settled(&self, constants: &Constants, shown: Shown) -> bool {
        let Some(key) = constants.planes_key(self.frame()) else { return true };
        match Tier::of(shown.zoom) {
            Tier::Fit if !constants.clone().without_fine_detail().has_local_tone() => true,
            Tier::Fit => self.grid.as_ref().is_some_and(|(held, _)| *held == key),
            Tier::Region => self.planes.key() == Some(&key),
        }
    }
}

fn color_output(engine: &Engine, width: u32, height: u32) -> PooledTexture {
    DngRenderPipeline::pooled_output(engine.gpu(), width, height)
}

/// `region` within a `frame`-sized picture, at least one pixel.
fn within(region: Region, [w, h]: [u32; 2]) -> Region {
    let x = region.x.min(w - 1);
    let y = region.y.min(h - 1);
    let width = region.width.clamp(1, w - x);
    let height = region.height.clamp(1, h - y);
    Region { x, y, width, height }
}

/// Constants that draw `stack` on the GPU: they name how its pixels are
/// uploaded, which only its profile decides.
fn source_constants(stack: &CompletedStack) -> Result<Box<Constants>> {
    let look = Look::redrawn(stack, &Adjustments::NEUTRAL, Reframe::NONE, OutputSpace::Srgb);
    match look.resolve() {
        Draw::Rendered(constants) => Ok(constants),
        Draw::Stored { .. } => Err(Error::InvalidState("a redrawn look draws on the GPU".into())),
    }
}

fn pixels(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height)
}

/// One open render's memory in bytes (ADR-114): on the GPU, what it keeps
/// and what a full redraw adds while it runs; on the CPU, what the app
/// holds of it at the same moment. On a Mac or an iPhone both are the same
/// memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderMemory {
    pub source: u64,
    pub reduced: u64,
    /// The local tone's planes, kept between draws (CPU).
    pub planes: u64,
    /// The fit tier's copies: the reduced copy at the size shown, the
    /// local tone's values on their grid and at that size.
    pub fit: u64,
    /// A region tick past half zoom: its output and its local tone's
    /// values, which a full redraw lets go first.
    pub region: u64,
    /// A full redraw's output, in flight.
    pub redraw: u64,
    /// A redraw's strip with local tone: its planes' values (GPU and CPU)
    /// and the log brightness around it (GPU and CPU).
    pub strip: u64,
    /// The stack's pixels, RGB u16 (CPU).
    pub pixels: u64,
    /// The two flat pictures of the preview's slot, the largest region's
    /// (CPU).
    pub flat: u64,
    /// The frame map, u16 (CPU).
    pub frame_map: u64,
    /// The display's levels, 8-bit RGBA with its halves, two during a swap
    /// (CPU).
    pub display: u64,
}

impl RenderMemory {
    /// A `width`×`height` render whose source and output take
    /// `texel` bytes a pixel, before its copies are made: the reduced copy
    /// at its size, no planes, the fit tier at its largest and a region
    /// tick just past half zoom on a [`VIEWPORT`] screen.
    pub fn of(width: u32, height: u32, texel: u64) -> Self {
        let n = pixels(width, height);
        let [rw, rh] = reduced_size(width, height);
        let reduced_texel = u64::from(REDUCED_FORMAT.block_copy_size(None).unwrap_or(8));
        let strip_rows = u64::from(height.min(TILE));
        let strip = u64::from(width) * strip_rows * (16 + 4) * 2;
        let [fw, fh] = fit_size(width, height, 1.0, [rw, rh]);
        let fitted = pixels(fw, fh);
        // Its copy and its values in 16-bit float, the grid in 32, its output.
        let fit = fitted * (reduced_texel * 2 + 16);
        let side = |screen: u32, frame: u32| (screen * 2).min(frame);
        let region = pixels(side(VIEWPORT[0], width), side(VIEWPORT[1], height));
        Self {
            source: texel * n,
            reduced: reduced_texel * pixels(rw, rh),
            planes: 0,
            fit,
            region: region * (texel + 16),
            redraw: texel * n,
            strip,
            pixels: 6 * n,
            flat: 2 * 4 * region.max(fitted),
            frame_map: 2 * n,
            display: 2 * (4 * n * 4 / 3),
        }
    }

    /// The GPU's share at its height: the copies, then a redraw and its
    /// strip or a region tick, whichever is larger, since a redraw lets
    /// the region's values go first. A fit tick's output is less than
    /// either.
    pub fn gpu(&self) -> u64 {
        self.source + self.reduced + self.fit + (self.redraw + self.strip / 2).max(self.region)
    }

    /// The CPU's share at the same moment.
    pub fn cpu(&self) -> u64 {
        self.planes + self.strip / 2 + self.pixels + self.flat + self.frame_map + self.display
    }
}

impl std::fmt::Display for RenderMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mb = |b: u64| b >> 20;
        write!(
            f,
            "GPU {} MB (source {}, reduced {}, fit {}, redraw {}), CPU {} MB (planes {})",
            mb(self.gpu()),
            mb(self.source),
            mb(self.reduced),
            mb(self.fit),
            mb(self.redraw),
            mb(self.cpu()),
            mb(self.planes)
        )
    }
}

/// The screen a budget is counted for: the first days' 100% case.
pub const VIEWPORT: [u32; 2] = [2560, 1600];

/// An open 45-megapixel render's GPU memory may reach this (ADR-114): its
/// copies, the fit tier's, and one full redraw and its strip,
/// Rgba16Unorm.
pub const GPU_BUDGET_45MP: u64 = 1280 << 20;

#[cfg(test)]
mod tests {
    use super::*;

    /// A 45-megapixel render, as the R5 II writes it.
    const R5: [u32; 2] = [8256, 5504];

    #[test]
    fn an_open_45mp_render_stays_in_its_gpu_budget() {
        let memory = RenderMemory::of(R5[0], R5[1], 8);
        assert_eq!(memory.reduced, 4096 * 2731 * 8);
        assert!(memory.gpu() <= GPU_BUDGET_45MP, "{memory}");
    }

    /// The real copies of a synthetic stack: their size, and the budget's
    /// figures for what they hold.
    #[test]
    fn copies_hold_what_the_budget_counts() {
        let Some(engine) = crate::test_support::engine() else { return };
        let (w, h) = (5000u32, 300u32);
        let pixels: Vec<u16> = (0..w * h * 3).map(|i| (i % 65_536) as u16).collect();
        let stack = CompletedStack::srgb(w, h, pixels.clone(), Vec::new());
        let copies = GpuCopies::make(&engine, &stack, &pixels, Generation::ON_DISK).unwrap();
        assert_eq!([copies.reduced.width(), copies.reduced.height()], [4096, 246]);
        let memory = copies.memory();
        let texel = u64::from(engine.gpu().source_format().block_copy_size(None).unwrap());
        assert_eq!(memory.source, texel * u64::from(w * h));
        assert_eq!(memory.reduced, 8 * 4096 * 246);
        assert_eq!(memory.planes, 0, "no draw has needed planes yet");
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod preview_tests;
