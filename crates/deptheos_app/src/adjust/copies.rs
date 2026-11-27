//! What an open render keeps on the GPU while Adjust is open (ADR-114):
//! the full-size source, the reduced copy the drag preview draws from, and
//! the local tone's planes, all made for one generation of the stored
//! picture with paint ([`crate::engine::generation`]). Never saved; exports
//! and thumbnails draw from the stored pixels.

use deptheos_gpu::PooledTexture;
use deptheos_render::dng_render::{
    Constants, KeptPlanes, REDUCED_FORMAT, Region, Source, TILE, reduced_size,
};
use deptheos_render::{Adjustments, Draw, OutputSpace, Reframe};

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
    planes: KeptPlanes,
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
        let reduced = engine.color().reduce(engine.gpu(), &source, &constants);
        engine.gpu().check().map_err(deptheos_core::CoreError::from)?;
        let copies = Self { generation, source, reduced, planes: KeptPlanes::default() };
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
    /// tone's planes kept for the next draw.
    pub fn draw_full(&mut self, engine: &Engine, constants: &Constants) -> PooledTexture {
        let (color, ctx) = (engine.color(), engine.gpu());
        let (width, height) = (self.source.width(), self.source.height());
        let source = Source::whole(&self.source);
        let band = height.min(TILE);
        let prepared = color.with_kept_planes(ctx, constants, source, band, &mut self.planes);
        let output = color_output(engine, width, height);
        color.draw(ctx, &prepared, source, Region::whole(width, height), &output);
        output
    }

    /// What they hold now and what a full redraw adds.
    pub fn memory(&self) -> RenderMemory {
        let (width, height) = (self.source.width(), self.source.height());
        let texel = |t: &wgpu::Texture| u64::from(t.format().block_copy_size(None).unwrap_or(16));
        RenderMemory {
            source: texel(&self.source) * pixels(width, height),
            reduced: texel(&self.reduced) * pixels(self.reduced.width(), self.reduced.height()),
            planes: self.planes.bytes(),
            ..RenderMemory::of(width, height, texel(&self.source))
        }
    }
}

fn color_output(engine: &Engine, width: u32, height: u32) -> PooledTexture {
    deptheos_render::dng_render::DngRenderPipeline::pooled_output(engine.gpu(), width, height)
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
    /// A full redraw's output, in flight.
    pub redraw: u64,
    /// A redraw's strip with local tone: its planes' values (GPU and CPU)
    /// and the log brightness around it (GPU and CPU).
    pub strip: u64,
    /// The stack's pixels, RGB u16 (CPU).
    pub pixels: u64,
    /// The frame map, u16 (CPU).
    pub frame_map: u64,
    /// The display's levels, 8-bit RGBA with its halves, two during a swap
    /// (CPU).
    pub display: u64,
}

impl RenderMemory {
    /// A `width`×`height` render whose source and output take
    /// `texel` bytes a pixel, before its copies are made: the reduced copy
    /// at its size and no planes.
    pub fn of(width: u32, height: u32, texel: u64) -> Self {
        let n = pixels(width, height);
        let [rw, rh] = reduced_size(width, height);
        let reduced_texel = u64::from(REDUCED_FORMAT.block_copy_size(None).unwrap_or(8));
        let strip_rows = u64::from(height.min(TILE));
        let strip = u64::from(width) * strip_rows * (16 + 4) * 2;
        Self {
            source: texel * n,
            reduced: reduced_texel * pixels(rw, rh),
            planes: 0,
            redraw: texel * n,
            strip,
            pixels: 6 * n,
            frame_map: 2 * n,
            display: 2 * (4 * n * 4 / 3),
        }
    }

    /// The GPU's share at its height: the copies, a redraw and its strip.
    pub fn gpu(&self) -> u64 {
        self.source + self.reduced + self.redraw + self.strip / 2
    }

    /// The CPU's share at the same moment.
    pub fn cpu(&self) -> u64 {
        self.planes + self.strip / 2 + self.pixels + self.frame_map + self.display
    }
}

impl std::fmt::Display for RenderMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mb = |b: u64| b >> 20;
        write!(
            f,
            "GPU {} MB (source {}, reduced {}, redraw {}), CPU {} MB (planes {})",
            mb(self.gpu()),
            mb(self.source),
            mb(self.reduced),
            mb(self.redraw),
            mb(self.cpu()),
            mb(self.planes)
        )
    }
}

/// An open 45-megapixel render's GPU memory may reach this (ADR-114): its
/// copies, one full redraw and its strip, Rgba16Unorm.
pub const GPU_BUDGET_45MP: u64 = 1200 << 20;

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
