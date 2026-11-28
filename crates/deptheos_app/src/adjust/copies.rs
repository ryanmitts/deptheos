//! What an open render keeps on the GPU while Adjust is open (ADR-114):
//! the full-size source, the reduced copy the drag preview draws from, and
//! the local tone's planes, all made for one generation of the stored
//! picture with paint ([`crate::engine::generation`]). Never saved; exports
//! and thumbnails draw from the stored pixels. The preview's two tiers
//! draw from them ([`GpuCopies::draw_preview`], ADR-138).

use deptheos_core::io;
use deptheos_gpu::PooledTexture;
use deptheos_gpu::gpu_utils::download_texture_with;
use deptheos_render::dng_render::{
    Constants, DngRenderPipeline, Encoding, GridPlanes, KeptPlanes, PlanesKey, PreparedRender,
    REDUCED_FORMAT, Region, Source, TILE, reduced_size,
};
use deptheos_render::{Adjustments, Draw, OutputSpace, Reframe};

use deptheos_render::reframe::{Rect, Size};
use image::RgbImage;

use super::histogram::{Histogram, SMALL_LONG, scaled_window};
use super::preview::{Draft, FlatPicture, FlatSlot, Shown, Tier, fit_size, read_flat};
use crate::engine::Engine;
use crate::engine::generation::Generation;
use crate::engine::present::source_rgb;
use crate::library::render_file::THUMBNAIL_SIDE;
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
    /// The fit tier's, at the size shown.
    fit: Fitted,
    /// The histogram's and the thumbnail's, at their sizes.
    small: Fitted,
    /// The local tone's values of the whole frame on a grid of the fit
    /// tier's largest size, gathered from the full-size pixels, and what
    /// their planes were made from.
    grid: Option<(PlanesKey, wgpu::Texture)>,
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
            fit: Fitted::default(),
            small: Fitted::default(),
            grid: None,
            region: None,
        };
        tracing::debug!("adjust: GPU copies made, {}", copies.memory());
        Ok(copies)
    }

    /// Let go of the preview's copies at the sizes drawn and the region's
    /// values, keeping the source, the reduced copy and the planes: the
    /// next tick makes them again.
    pub fn trim_preview(&mut self) {
        self.fit = Fitted::default();
        self.small = Fitted::default();
        self.region = None;
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

    /// The histogram of the picture through `constants` under `crop`
    /// (stored pixels), counted from a draw of the reduced copy
    /// [`SMALL_LONG`] on its long side, so it doesn't change with zoom or
    /// pan. Its clipping is the small draw's; [`super::histogram::clipping`]
    /// counts it again over a full redraw.
    pub fn histogram(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        crop: Option<Rect>,
        draft: Draft,
    ) -> Result<Histogram> {
        let [w, h] = self.frame();
        let zoom = SMALL_LONG as f32 / w.max(h) as f32;
        let size = fit_size(w, h, zoom, [self.reduced.width(), self.reduced.height()]);
        let output = self.draw_fitted(engine, constants, size, draft, Fit::Small);
        let window = scaled_window(crop, [w, h], size);
        let rgb = read_rgb8(engine, &output, window)?;
        let row = window.width as usize * 3;
        Ok(Histogram::count(rgb.chunks_exact(row)))
    }

    /// The picture through `constants` (a look's for a file, framed by
    /// its reframe) as a thumbnail: the reframe's window, flipped and
    /// turned, [`THUMBNAIL_SIDE`] on its long side or its full size if
    /// smaller. Drawn from the reduced copy, without Texture and
    /// Sharpening, unless the window is too small in it; then the window
    /// is drawn at full size with every slider and reduced on the CPU.
    pub fn thumbnail(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        reframe: Reframe,
    ) -> Result<RgbImage> {
        let [w, h] = self.frame();
        let picture = Size { width: w, height: h };
        let crop = reframe
            .window(picture)
            .map_err(|o| Error::InvalidState(format!("crop {:?} outside the picture", o.crop)))?;
        let long = crop.width.max(crop.height);
        let scale = (THUMBNAIL_SIDE as f32 / long as f32).min(1.0);
        let size = fit_size(w, h, scale, [self.reduced.width(), self.reduced.height()]);
        let reached = size[0].max(size[1]) as f32 / w.max(h) as f32;
        let (rgb, drawn) = if reached >= scale * 0.999 {
            let output = self.draw_fitted(engine, constants, size, Draft::Settled, Fit::Small);
            let window = scaled_window(Some(crop), [w, h], size);
            (read_rgb8(engine, &output, window)?, [window.width, window.height])
        } else {
            let region = Region { x: crop.x, y: crop.y, width: crop.width, height: crop.height };
            let output = self.region_texture(engine, constants, region, Draft::Settled);
            let whole = Region::whole(crop.width, crop.height);
            (read_rgb8(engine, &output, whole)?, [crop.width, crop.height])
        };
        let image = RgbImage::from_raw(drawn[0], drawn[1], rgb).expect("rows of its size");
        let target = |v: u32| ((v as f32 * scale).round() as u32).max(1);
        let (tw, th) = (target(crop.width), target(crop.height));
        let image =
            if [tw, th] == drawn { image } else { image::imageops::thumbnail(&image, tw, th) };
        let turned = Reframe { crop: None, ..reframe };
        let (pixels, out) = turned
            .apply(image.as_raw(), Size { width: tw, height: th }, 3)
            .expect("the whole picture is inside it");
        Ok(RgbImage::from_raw(out.width, out.height, pixels).expect("rows of its size"))
    }

    fn draw_fit(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        zoom: f32,
        draft: Draft,
        slot: &FlatSlot,
    ) -> Result<FlatPicture> {
        let [w, h] = self.frame();
        let size = fit_size(w, h, zoom, [self.reduced.width(), self.reduced.height()]);
        let output = self.draw_fitted(engine, constants, size, draft, Fit::Shown);
        read_flat(engine.gpu(), &output, size, Region::whole(w, h), constants.output(), slot)
    }

    /// The whole picture through `constants` at `size`, from the reduced
    /// copy, without Texture and Sharpening, into `which` copies.
    fn draw_fitted(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        size: [u32; 2],
        draft: Draft,
        which: Fit,
    ) -> PooledTexture {
        let (color, ctx) = (engine.color(), engine.gpu());
        let [w, h] = self.frame();
        if self.fitted(which).copy.as_ref().is_none_or(|(s, _)| *s != size) {
            let copy = color.resample(ctx, &self.reduced, size, &self.encoding);
            self.fitted(which).copy = Some((size, copy));
        }
        let (sx, sy) = (size[0] as f32 / w as f32, size[1] as f32 / h as f32);
        let drawn = constants.clone().without_fine_detail().reduced().scaled(sx, sy);
        if drawn.has_local_tone() {
            self.fit_planes(engine, constants, size, draft, which);
        }
        let fitted = self.fitted(which);
        let (_, copy) = fitted.copy.as_ref().expect("made above");
        let prepared = PreparedRender::new(ctx, &drawn);
        let (output, whole) =
            (color_output(engine, size[0], size[1]), Region::whole(size[0], size[1]));
        match &fitted.planes {
            Some((_, _, planes)) if drawn.has_local_tone() => {
                color.draw_reading(ctx, &prepared, Source::whole(copy), whole, planes, &output);
            }
            _ => color.draw(ctx, &prepared, Source::whole(copy), whole, &output),
        }
        output
    }

    fn fitted(&mut self, which: Fit) -> &mut Fitted {
        match which {
            Fit::Shown => &mut self.fit,
            Fit::Small => &mut self.small,
        }
    }

    fn draw_region(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        region: Region,
        draft: Draft,
        slot: &FlatSlot,
    ) -> Result<FlatPicture> {
        let output = self.region_texture(engine, constants, region, draft);
        let size = [region.width, region.height];
        read_flat(engine.gpu(), &output, size, region, constants.output(), slot)
    }

    /// `region` of the picture through `constants` at full size, every
    /// slider.
    fn region_texture(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        region: Region,
        draft: Draft,
    ) -> PooledTexture {
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
        output
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
    fn fit_planes(
        &mut self,
        engine: &Engine,
        constants: &Constants,
        size: [u32; 2],
        draft: Draft,
        which: Fit,
    ) {
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
        let held = match which {
            Fit::Shown => &self.fit.planes,
            Fit::Small => &self.small.planes,
        };
        if !held.as_ref().is_some_and(|(k, s, _)| k == grid_key && *s == size) {
            let resampled = (grid_key.clone(), size, color.resample_planes(ctx, grid, size));
            self.fitted(which).planes = Some(resampled);
        }
    }

    fn set_grid(&mut self, key: PlanesKey, grid: wgpu::Texture) {
        self.fit.planes = None;
        self.small.planes = None;
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
            fit: [&self.fit, &self.small]
                .into_iter()
                .flat_map(|f| {
                    [f.copy.as_ref().map(|(_, t)| t), f.planes.as_ref().map(|(_, _, t)| t)]
                })
                .chain([self.grid.as_ref().map(|(_, t)| t)])
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

/// The reduced copy at one size, and the local tone's values at it.
#[derive(Default)]
struct Fitted {
    copy: Option<([u32; 2], wgpu::Texture)>,
    planes: Option<(PlanesKey, [u32; 2], wgpu::Texture)>,
}

/// Which [`Fitted`] a draw uses: the fit tier's, or the small draws'.
#[derive(Clone, Copy)]
enum Fit {
    Shown,
    Small,
}

/// `window` of `texture`, a drawn output, as 8-bit RGB rows.
fn read_rgb8(engine: &Engine, texture: &wgpu::Texture, window: Region) -> Result<Vec<u8>> {
    let texel = match texture.format() {
        wgpu::TextureFormat::Rgba16Unorm => 8,
        wgpu::TextureFormat::Rgba32Float => 16,
        other => return Err(Error::InvalidState(format!("cannot read a {other:?} texture"))),
    };
    let mut rgb = Vec::with_capacity(window.width as usize * window.height as usize * 3);
    let (origin, size) = ([window.x, window.y], [window.width, window.height]);
    download_texture_with(engine.gpu(), texture, origin, size, |row| {
        for t in row.chunks_exact(texel) {
            rgb.extend(source_rgb(t).map(io::u16_to_u8));
        }
    })
    .map_err(|e| Error::Core(e.into()))?;
    Ok(rgb)
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

    /// What Adjust adds to the app's memory at its height, beyond the
    /// stack's pixels, frame map and display it already holds: the GPU's
    /// share, a strip's values read back, the preview's flat pictures and
    /// the second display of a swap. On iOS, where the GPU's memory is the
    /// app's, an open is refused unless this fits what is left.
    pub fn added(&self) -> u64 {
        self.gpu() + self.strip / 2 + self.flat + self.display / 2
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
            "GPU {} MB (source {}, reduced {}, fit {}, redraw {}), CPU {} MB (planes {}), {} MB added",
            mb(self.gpu()),
            mb(self.source),
            mb(self.reduced),
            mb(self.fit),
            mb(self.redraw),
            mb(self.cpu()),
            mb(self.planes),
            mb(self.added())
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

    /// What an open adds on iOS: under 2 GiB at 45 MP, past 3 GiB at
    /// 100 MP, which an 8 GB iPhone's live budget beside the stack may refuse.
    #[test]
    fn what_an_open_adds_counts_the_gpu_and_what_the_cpu_newly_holds() {
        let at_45 = RenderMemory::of(R5[0], R5[1], 8);
        assert!(at_45.added() > at_45.gpu() && at_45.added() < 2 << 30, "{at_45}");
        let at_100 = RenderMemory::of(12_240, 8_160, 8);
        assert!(at_100.added() > 3 << 30, "{at_100}");
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
