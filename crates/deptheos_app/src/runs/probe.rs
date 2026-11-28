//! What the sliders that read neighbors cost in memory, measured on the
//! device that runs it: a synthetic standard stack drawn and read back as
//! an export does, without them, with them all whole, or with them all a
//! band at a time, one
//! draw per process, since memory let go of isn't always returned. The
//! memory probe example runs it on the desktop; a debug iOS build runs it
//! when its launch environment asks (`DEPTHEOS_PROBE_LOCAL_TONE`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use deptheos_core::upload::ImageUploader;
use deptheos_gpu::GpuContext;
use deptheos_render::dng_render::DngRenderPipeline;
use deptheos_render::{Adjustments, Color, OutputSpace, Sharpening, resolve_adjusted};
use image::math::Rect;

use crate::Result;
use crate::engine::memory;
use crate::runs::session::readback::download_rgb;

/// One step's time and the process footprint before it, at its peak
/// (sampled each millisecond, GPU memory included) and after it.
#[derive(Clone, Debug)]
pub struct Step {
    pub what: &'static str,
    pub millis: u64,
    pub before: u64,
    pub peak: u64,
    pub after: u64,
}

/// Which draw the probe measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draw {
    Without,
    Whole,
    /// Read and drawn this many rows at a time.
    Bands(u32),
}

impl Draw {
    /// `without`, `whole`, or `bands` with a band of `band` rows.
    pub fn named(name: &str, band: u32) -> Option<Self> {
        match name {
            "without" => Some(Self::Without),
            "whole" => Some(Self::Whole),
            "bands" => Some(Self::Bands(band)),
            _ => None,
        }
    }
}

/// `draw` of a `width`×`height` stack, measured.
pub fn local_tone_memory(ctx: &GpuContext, width: u32, height: u32, draw: Draw) -> Result<Step> {
    let pixels: Vec<u16> = (0..width as usize * height as usize * 3)
        .map(|i| {
            let (x, y) = (i / 3 % width as usize, i / 3 / width as usize);
            let block = if (x / 900 + y / 700) % 2 == 0 { 6000 } else { 48_000 };
            (block + (i.wrapping_mul(2_654_435_761) >> 20) % 4000 + i % 3 * 1500) as u16
        })
        .collect();
    let pipeline = DngRenderPipeline::new(ctx);
    let constants = |adjustments: &Adjustments| {
        *resolve_adjusted(Color::Standard(None), adjustments, OutputSpace::Srgb)
    };
    let plain = constants(&Adjustments { exposure: 0.3, ..Adjustments::NEUTRAL });
    let toned = constants(&Adjustments {
        exposure: 0.3,
        highlights: -60.0,
        shadows: 60.0,
        clarity: 30.0,
        texture: 20.0,
        sharpening: Sharpening { amount: 40.0, ..Sharpening::NONE },
        ..Adjustments::NEUTRAL
    });
    let whole = Rect { x: 0, y: 0, width, height };
    let upload = || {
        let mut uploader = ImageUploader::new(ctx);
        let prep = uploader.prepare_rgb16(ctx, &pixels, width, height)?;
        let texture = ImageUploader::create_target(ctx, width, height, "probe_source");
        let mut encoder = ctx.encoder("probe_upload");
        prep.dispatch_to_rgba(ctx, &mut encoder, &texture);
        ctx.submit(encoder);
        Ok::<_, crate::Error>(texture)
    };
    let render = |constants| {
        let input = upload()?;
        let output = pipeline.render(ctx, &input, constants);
        download_rgb(ctx, &output, whole).map(|v| v.len())
    };
    match draw {
        Draw::Without => measure("without them, whole", || render(&plain)),
        Draw::Whole => measure("with them, whole", || render(&toned)),
        Draw::Bands(band) => measure("with them, in bands", || {
            let input = upload()?;
            let mut out = vec![0u16; pixels.len()];
            let mut failed = None;
            pipeline.render_bands(ctx, &input, &toned, band, |region, texture| {
                let rect = Rect { x: 0, y: 0, width: region.width, height: region.height };
                match download_rgb(ctx, texture, rect) {
                    Ok(rows) => {
                        let at = region.y as usize * width as usize * 3;
                        out[at..at + rows.len()].copy_from_slice(&rows);
                    }
                    Err(e) => failed = Some(e),
                }
            });
            failed.map_or(Ok(out.len()), Err)
        }),
    }
}

fn measure<T>(what: &'static str, step: impl FnOnce() -> Result<T>) -> Result<Step> {
    let footprint = || memory::footprint().unwrap_or(0);
    let before = footprint();
    let started = Instant::now();
    let done = AtomicBool::new(false);
    let (out, peak) = std::thread::scope(|s| {
        let sampler = s.spawn(|| {
            let mut top = 0;
            while !done.load(Ordering::Relaxed) {
                top = top.max(footprint());
                std::thread::sleep(Duration::from_millis(1));
            }
            top
        });
        let out = step();
        done.store(true, Ordering::Relaxed);
        (out, sampler.join().unwrap_or(0))
    });
    out?;
    let millis = started.elapsed().as_millis() as u64;
    let step = Step { what, millis, before, peak, after: footprint() };
    tracing::info!(
        "probe: {what} in {millis} ms, footprint {} MB, peak {} MB, after {} MB",
        before >> 20,
        peak >> 20,
        step.after >> 20
    );
    Ok(step)
}

/// One drag tick of Adjust on a synthetic standard stack, timed: the GPU
/// copies made once, then each tick drawn and read back as the session
/// draws it, so an app can time the rest of its way to the screen
/// (`DEPTHEOS_PROBE_ADJUST_TICK` in the iOS app's debug build).
pub struct AdjustTicks {
    stack: crate::CompletedStack,
    copies: crate::adjust::GpuCopies,
    slot: crate::adjust::preview::FlatSlot,
    base: Adjustments,
}

impl AdjustTicks {
    /// A `width`×`height` stack of fine detail, its copies made, with
    /// Highlights, Shadows and Clarity set when `local`.
    pub fn new(engine: &crate::Engine, width: u32, height: u32, local: bool) -> Result<Self> {
        let pixels: Vec<u16> = (0..width as usize * height as usize * 3)
            .map(|i| {
                let (x, y) = (i / 3 % width as usize, i / 3 / width as usize);
                let line = x % 7 == 0 || y % 11 == 0;
                let base = 0.25 + 0.6 * x as f32 / width as f32;
                let v = if line { 0.04 } else { base * (0.9 + 0.1 * (i % 3) as f32) };
                (v * 65535.0) as u16
            })
            .collect();
        let stack = crate::CompletedStack::srgb(width, height, pixels, Vec::new());
        let generation = crate::engine::generation::Generation::ON_DISK;
        let copies = crate::adjust::GpuCopies::make(engine, &stack, &stack.pixels, generation)?;
        let per_pixel = Adjustments { contrast: 20.0, saturation: 10.0, ..Adjustments::NEUTRAL };
        let base = if local {
            Adjustments { highlights: -30.0, shadows: 40.0, clarity: 25.0, ..per_pixel }
        } else {
            per_pixel
        };
        let mut ticks = Self { stack, copies, slot: Default::default(), base };
        // The first draw makes the planes a drag then reads.
        let (first, _) = ticks.draw(engine, 0, None, crate::adjust::preview::Draft::Settled)?;
        ticks.slot.give_back(first);
        Ok(ticks)
    }

    /// Tick `step` (each a little more exposure): at fit on a screen
    /// `screen` pixels on its long side, or with `region` the part of
    /// that size at 100%. The picture and the draw's time, readback
    /// included.
    pub fn tick(
        &mut self,
        engine: &crate::Engine,
        step: u32,
        screen: [u32; 2],
        region: bool,
    ) -> Result<(crate::adjust::preview::FlatPicture, Duration)> {
        let shown = Some((screen, region));
        self.draw(engine, step, shown, crate::adjust::preview::Draft::Tick)
    }

    /// The full picture a drag settles on, at tick `step`, drawn as the
    /// session draws it (the whole redraw, its clipping counted and its
    /// display made), measured with the footprint's peak.
    pub fn settle(&mut self, engine: &crate::Engine, step: u32) -> Result<Step> {
        let value = Adjustments { exposure: 0.01 * step as f64, ..self.base.clone() };
        let look = crate::Look::adjusting(&self.stack, &value, deptheos_render::Reframe::NONE);
        let Some(constants) = look.constants() else {
            return Err(crate::Error::InvalidState("Adjust draws on the GPU".into()));
        };
        let copies = &mut self.copies;
        measure("a settle's full picture", || {
            let full = copies.draw_full(engine, &constants);
            crate::adjust::histogram::clipping(engine, &full, None)?;
            let shown = engine
                .present(&full, &crate::engine::present::ToDisplay::drawn(constants.output()))?;
            drop(full);
            Ok(shown)
        })
    }

    /// The open render's GPU copies, as counted.
    pub fn memory(&self) -> crate::adjust::RenderMemory {
        self.copies.memory()
    }

    /// A picture's buffer, for the next tick.
    pub fn give_back(&self, picture: crate::adjust::preview::FlatPicture) {
        self.slot.give_back(picture);
    }

    fn draw(
        &mut self,
        engine: &crate::Engine,
        step: u32,
        shown: Option<([u32; 2], bool)>,
        draft: crate::adjust::preview::Draft,
    ) -> Result<(crate::adjust::preview::FlatPicture, Duration)> {
        use crate::adjust::preview::Shown;
        use deptheos_render::dng_render::Region;
        let (w, h) = (self.stack.width, self.stack.height);
        let ([sw, sh], region) = shown.unwrap_or(([2048, 2048], false));
        let shown = if region {
            let (vw, vh) = (sw.min(w), sh.min(h));
            let visible = Region { x: (w - vw) / 2, y: (h - vh) / 2, width: vw, height: vh };
            Shown { zoom: 1.0, visible }
        } else {
            Shown { zoom: sw.max(sh) as f32 / w.max(h) as f32, visible: Region::whole(w, h) }
        };
        let value = Adjustments { exposure: 0.01 * step as f64, ..self.base.clone() };
        let look = crate::Look::adjusting(&self.stack, &value, deptheos_render::Reframe::NONE);
        let Some(constants) = look.constants() else {
            return Err(crate::Error::InvalidState("Adjust draws on the GPU".into()));
        };
        let started = Instant::now();
        let picture = self.copies.draw_preview(engine, &constants, shown, draft, &self.slot)?;
        Ok((picture, started.elapsed()))
    }
}
