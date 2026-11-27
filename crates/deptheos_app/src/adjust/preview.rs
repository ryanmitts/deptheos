//! The drag preview (ADR-138): each tick one flat picture, drawn at fit
//! from the reduced copy or past 50% zoom from the full-size source over
//! the region on screen, read back into a reused slot.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deptheos_core::io;
use deptheos_gpu::gpu_utils::download_texture_rows;
use deptheos_render::OutputSpace;
pub use deptheos_render::dng_render::Region;
use rayon::prelude::*;

use crate::engine::orient::Orient;
use crate::engine::present::{DISPLAY_FORMAT, PixelRect, display_pixel, source_rgb};
use crate::{Error, Result};

/// The zoom (screen pixels per stored pixel) past which the preview draws
/// the region on screen from the full-size source.
pub const REGION_ZOOM: f32 = 0.5;

/// The fit preview's longest side.
pub const FIT_LONG: u32 = 2048;

/// Which picture a tick draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The whole picture at the size shown, from the reduced copy, with
    /// neither Texture nor Sharpening.
    Fit,
    /// The region on screen at full size, every slider.
    Region,
}

impl Tier {
    /// The tier for `zoom`.
    pub fn of(zoom: f32) -> Self {
        if zoom > REGION_ZOOM { Self::Region } else { Self::Fit }
    }

    /// A tick's budget (owner, 2026-11-26): 16 ms, or 33 ms for a region
    /// with Texture or Sharpening set.
    pub fn budget(self, fine_detail: bool) -> Duration {
        match (self, fine_detail) {
            (Self::Region, true) => Duration::from_millis(33),
            _ => Duration::from_millis(16),
        }
    }
}

/// The settled picture's budget: on screen this long after a drag ends
/// at 45 MP (ADR-138).
pub const SETTLE_BUDGET: Duration = Duration::from_millis(250);

/// What the viewer shows, in stored pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    /// Screen pixels per stored pixel.
    pub zoom: f32,
    /// The part of the stored picture on screen.
    pub visible: Region,
}

/// Whether a draw is a drag's or the settled picture's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Draft {
    /// A tick: the local tone's planes kept from an earlier value are
    /// read while they are of the same sliders, though a white balance or
    /// a profile change since would make them anew.
    Tick,
    /// The settled picture: planes made for exactly this value.
    Settled,
}

/// One flat picture handed to an app: [`DISPLAY_FORMAT`] pixels, row by
/// row, covering `covers` of the stored picture (turned and flipped as
/// the render's reframe says once [`Self::oriented`]), their colors in
/// `space`.
pub struct FlatPicture {
    pub width: u32,
    pub height: u32,
    /// The stored pixels it shows: the whole picture at fit.
    pub covers: Region,
    /// sRGB, or a standard stack's own profile with Color Management off.
    pub space: OutputSpace,
    pub format: wgpu::TextureFormat,
    pub pixels: Vec<u8>,
}

impl FlatPicture {
    /// Turned and flipped by `orient`, `covers` then in the turned
    /// picture's pixels, into a buffer from `slot` (this one given back).
    pub fn oriented(self, orient: &Orient, slot: &FlatSlot) -> Self {
        if orient.is_none() {
            return self;
        }
        let Region { x, y, width, height } = self.covers;
        let shown = orient.to_shown(PixelRect { x, y, width, height });
        let size = deptheos_render::reframe::Size { width: self.width, height: self.height };
        let mut out = slot.take(self.pixels.len());
        orient.write(&self.pixels, size, &mut out);
        let turned = orient.turned(size);
        let picture = Self {
            width: turned.width,
            height: turned.height,
            covers: Region { x: shown.x, y: shown.y, width: shown.width, height: shown.height },
            space: self.space,
            format: self.format,
            pixels: out,
        };
        slot.give_back(self);
        picture
    }
}

/// The flat pictures' pixel buffers, handed back by the app and reused,
/// so a tick allocates none once the slot holds one of its size. Clones
/// share one slot.
#[derive(Clone, Default)]
pub struct FlatSlot {
    free: Arc<Mutex<Vec<Vec<u8>>>>,
    allocations: Arc<AtomicU64>,
}

impl FlatSlot {
    /// A buffer of `len` bytes: a handed-back one when it holds that many.
    fn take(&self, len: usize) -> Vec<u8> {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = free.iter().position(|b| b.capacity() >= len) {
            let mut buffer = free.swap_remove(i);
            buffer.resize(len, 0);
            return buffer;
        }
        self.allocations.fetch_add(1, Ordering::Relaxed);
        vec![0; len]
    }

    /// `picture`'s buffer, for the next tick. The app hands back each
    /// picture it has replaced on screen.
    pub fn give_back(&self, picture: FlatPicture) {
        let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
        // Three are enough: the one on screen, the one being drawn and the
        // one it is turned into.
        if free.len() < 3 {
            free.push(picture.pixels);
        }
    }

    /// How many buffers it has made.
    pub fn allocations(&self) -> u64 {
        self.allocations.load(Ordering::Relaxed)
    }
}

/// The size the fit tier draws a `width`×`height` picture at `zoom`: its
/// size on screen, at most [`FIT_LONG`] and the reduced copy's.
pub fn fit_size(width: u32, height: u32, zoom: f32, reduced: [u32; 2]) -> [u32; 2] {
    let long = width.max(height) as f32;
    let limit = (FIT_LONG.min(reduced[0].max(reduced[1])) as f32).min(long);
    let scale = zoom.clamp(f32::MIN_POSITIVE, 1.0).min(limit / long);
    let side = |v: u32, cap: u32| ((v as f32 * scale).round() as u32).clamp(1, cap);
    [side(width, reduced[0]), side(height, reduced[1])]
}

/// `texture`, drawn in `space`, read back as one flat picture covering
/// `covers`, into a buffer from `slot`.
pub(crate) fn read_flat(
    ctx: &deptheos_gpu::GpuContext,
    texture: &wgpu::Texture,
    size: [u32; 2],
    covers: Region,
    space: OutputSpace,
    slot: &FlatSlot,
) -> Result<FlatPicture> {
    let texel = match texture.format() {
        wgpu::TextureFormat::Rgba16Unorm => 8,
        wgpu::TextureFormat::Rgba32Float => 16,
        other => return Err(Error::InvalidState(format!("cannot display a {other:?} texture"))),
    };
    let [w, h] = size;
    let row = w as usize * 4;
    let mut pixels = slot.take(row * h as usize);
    let mut at = 0;
    download_texture_rows(ctx, texture, [0, 0], size, |_, mapped, stride, bytes| {
        let rows = mapped.len() / stride;
        let dst = &mut pixels[at..][..rows * row];
        dst.par_chunks_mut(row).zip(mapped.par_chunks(stride)).for_each(|(d, s)| {
            let texels = s[..bytes].chunks_exact(texel);
            for (d, t) in d.as_chunks_mut::<4>().0.iter_mut().zip(texels) {
                *d = display_pixel(source_rgb(t).map(io::u16_to_u8));
            }
        });
        at += rows * row;
    })
    .map_err(|e| Error::Core(e.into()))?;
    Ok(FlatPicture { width: w, height: h, covers, space, format: DISPLAY_FORMAT, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_turns_past_half_zoom() {
        assert_eq!(Tier::of(0.25), Tier::Fit);
        assert_eq!(Tier::of(0.5), Tier::Fit);
        assert_eq!(Tier::of(0.51), Tier::Region);
        assert_eq!(Tier::of(2.0), Tier::Region);
        assert_eq!(Tier::Region.budget(true), Duration::from_millis(33));
        assert_eq!(Tier::Region.budget(false), Duration::from_millis(16));
        assert_eq!(Tier::Fit.budget(true), Duration::from_millis(16));
    }

    #[test]
    fn the_fit_size_is_the_size_shown_at_most_2048() {
        let reduced = [4096, 2731];
        assert_eq!(fit_size(8256, 5504, 0.1, reduced), [826, 550]);
        assert_eq!(fit_size(8256, 5504, 0.5, reduced), [2048, 1365]);
        assert_eq!(fit_size(1000, 500, 0.5, [1000, 500]), [500, 250]);
        assert_eq!(fit_size(1000, 500, 0.0, [1000, 500]), [1, 1]);
    }

    #[test]
    fn a_handed_back_buffer_is_reused() {
        let slot = FlatSlot::default();
        let picture = |pixels| FlatPicture {
            width: 2,
            height: 2,
            covers: Region::whole(2, 2),
            space: OutputSpace::Srgb,
            format: DISPLAY_FORMAT,
            pixels,
        };
        let first = slot.take(16);
        slot.give_back(picture(first));
        let second = slot.take(12);
        assert_eq!((second.len(), slot.allocations()), (12, 1));
        slot.give_back(picture(second));
        assert_eq!(slot.take(64).len(), 64);
        assert_eq!(slot.allocations(), 2, "a larger picture needs a larger buffer");
    }
}
