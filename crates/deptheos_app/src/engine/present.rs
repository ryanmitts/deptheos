//! Display images, built on the CPU from pixels it holds or one readback.
//! The apps draw on their own devices, so pixels cross once per result
//! (ADR-100); pan and zoom are layout.
//!
//! Each image has half-size levels (2×2 box averages), since GPUI samples
//! bilinearly without mipmaps. Each level is cut into tiles: a
//! [`TILE_CORE`] px core plus a 1 px gutter of neighboring pixels, so
//! sampling at a core's edge matches a whole image, and every image fits
//! GPUI's atlas (four tiles fill a 1024² page).
//!
//! The display is sRGB or Display P3, as the app's screen is
//! ([`set_screen`]): pixels in another space are converted here, clipping
//! colors outside it on screen only ([`set_color_managed`]). Every image
//! names the space it is in ([`PresentedImage::space`]), which the app tags
//! it with or converts it from.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use deptheos_core::{FramePlane, PlaneRamp, UNLIT, io};
use deptheos_gpu::GpuContext;
use deptheos_gpu::gpu_utils::download_texture_rows;
use deptheos_render::OutputSpace;
use deptheos_render::reframe::Size;
use image::GrayImage;
use rayon::prelude::*;

use crate::engine::colormap::PLASMA;
use crate::runs::session::Gray16Image;
use crate::{Error, Result};

/// The display's channel order (GPUI's and Core Graphics').
pub(crate) const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

/// Packed opaque 8-bit pixels in [`DISPLAY_FORMAT`].
#[derive(Clone)]
pub struct CpuImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl CpuImage {
    /// `rgb` in the display's channel order.
    pub fn from_rgb(rgb: &image::RgbImage) -> Self {
        let (width, height) = rgb.dimensions();
        let mut pixels = Vec::with_capacity(rgb.as_raw().len() / 3 * 4);
        pixels.extend(rgb.pixels().flat_map(|p| display_pixel(p.0)));
        Self { width, height, pixels }
    }

    /// [`Self::from_rgb`]'s inverse.
    pub(crate) fn to_rgb(&self) -> image::RgbImage {
        let mut rgb = Vec::with_capacity(self.pixels.len() / 4 * 3);
        rgb.extend(self.pixels.as_chunks::<4>().0.iter().flat_map(|&p| rgb_of_display(p)));
        image::RgbImage::from_raw(self.width, self.height, rgb)
            .expect("four bytes a pixel make three")
    }
}

/// Side of the part of a level a tile covers.
pub const TILE_CORE: u32 = 510;

/// One level cut into row-major tiles. Tile `(tx, ty)` covers up to
/// [`TILE_CORE`] pixels a side from `(tx, ty) * TILE_CORE` and stores
/// `core + 2` a side, with its gutter.
#[derive(Clone)]
pub struct TiledLevel {
    pub width: u32,
    pub height: u32,
    pub cols: u32,
    pub rows: u32,
    pub tiles: Vec<CpuImage>,
}

impl TiledLevel {
    /// The level as one image again, gutters dropped.
    pub fn assemble(&self) -> CpuImage {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut pixels = vec![0u8; w * h * 4];
        for (i, tile) in self.tiles.iter().enumerate() {
            let (tx, ty) = (i % self.cols as usize, i / self.cols as usize);
            let (x0, y0) = (tx * TILE_CORE as usize, ty * TILE_CORE as usize);
            let (cw, ch) = (tile.width as usize - 2, tile.height as usize - 2);
            for y in 0..ch {
                let src = &tile.pixels[((y + 1) * tile.width as usize + 1) * 4..][..cw * 4];
                pixels[((y0 + y) * w + x0) * 4..][..cw * 4].copy_from_slice(src);
            }
        }
        CpuImage { width: self.width, height: self.height, pixels }
    }
}

/// An image and its half-size levels: `levels[k]` is at
/// `1 / 2^(first_level + k)` scale, each drawn over the full-size rect, so a
/// chain may start below full size (the map starts at half).
#[derive(Clone)]
pub struct PresentedImage {
    width: u32,
    height: u32,
    pub first_level: u32,
    pub levels: Vec<TiledLevel>,
    space: ScreenSpace,
}

impl PresentedImage {
    /// A single full-size level, no reductions, in sRGB.
    pub fn single(image: CpuImage) -> Self {
        let (width, height) = (image.width, image.height);
        let plane = Plane { width: width as usize, height: height as usize, pixels: image.pixels };
        let levels = vec![tile(&plane)];
        Self { width, height, first_level: 0, levels, space: ScreenSpace::Srgb }
    }

    /// `first` as level `first_level` of a `full`-sized picture in `space`,
    /// its smaller levels made from it, `count` levels in all.
    pub(crate) fn from_first(
        first: CpuImage,
        count: usize,
        first_level: u32,
        full: Size,
        space: ScreenSpace,
    ) -> Self {
        let (w, h) = (first.width as usize, first.height as usize);
        let mut levels = Levels::new(w, h, count.max(1));
        for band in first.pixels.chunks(BAND_ROWS * w * 4) {
            levels.fill(band.len() / (w * 4), |rows| rows.copy_from_slice(band));
        }
        let made = levels.finish();
        let (width, height) = (full.width, full.height);
        Self { width, height, first_level, levels: made.levels, space }
    }

    /// The space its pixels are in, which the app tags it with.
    pub fn space(&self) -> ScreenSpace {
        self.space
    }

    /// It, its pixels said to be in `space`.
    pub(crate) fn in_space(self, space: ScreenSpace) -> Self {
        Self { space, ..self }
    }

    /// Full-size width, whatever the first level's.
    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Drops the full-size level when there is a smaller one.
    pub fn below_full_size(mut self) -> Self {
        if self.first_level == 0 && self.levels.len() > 1 {
            self.levels.remove(0);
            self.first_level = 1;
        }
        self
    }

    /// Bytes of pixels held, for memory budgets.
    pub fn bytes(&self) -> usize {
        self.levels.iter().flat_map(|l| &l.tiles).map(|t| t.pixels.len()).sum()
    }
}

/// The level to draw at `scale` screen pixels per full-size pixel: the
/// largest still minified, so bilinear sampling never spans more than 2×2
/// source pixels; level 0 when magnified.
pub fn level_index(scale: f32, first_level: u32, count: usize) -> usize {
    if scale >= 1.0 || scale <= 0.0 {
        return 0;
    }
    let k = (1.0 / scale).log2().floor() as usize;
    k.saturating_sub(first_level as usize).min(count.saturating_sub(1))
}

/// `width`×`height` scaled down to `long_edge` on its long side, at least
/// 1×1, sides rounded half up in integer math (a float can round .5 down).
/// Unchanged when already within it or `long_edge` is 0. Thumbnails,
/// sequence frames and iOS's display levels all fit through this.
pub fn fit_long_edge(width: u32, height: u32, long_edge: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= long_edge || long_edge == 0 {
        return (width, height);
    }
    let (long, edge) = (u64::from(long), u64::from(long_edge));
    let side = |s: u32| ((u64::from(s) * edge + long / 2) / long).max(1) as u32;
    (side(width), side(height))
}

/// The display pixel of each 8-bit map value, in [`PLASMA`], first frame
/// accumulated to last.
fn plasma_table() -> [[u8; 4]; 256] {
    std::array::from_fn(|v| display_pixel(PLASMA[v]))
}

/// The legend color of frame-map value `v` (plane × 65535), interpolated
/// in [`PLASMA`] so stacks longer than the table keep distinct colors.
pub(crate) fn frame_map_color(v: u16) -> [u8; 3] {
    let t = f32::from(v) / 65535.0 * 255.0;
    let i = (t as usize).min(254);
    let f = t - i as f32;
    let (a, b) = (PLASMA[i], PLASMA[i + 1]);
    std::array::from_fn(|c| {
        (f32::from(a[c]) + (f32::from(b[c]) - f32::from(a[c])) * f).round() as u8
    })
}

/// The depth-map color of the `accumulated`th frame (from 0) of `total`.
/// Fixed from the start of a run (a frame left out keeps its plane), so
/// [`running_legend`] and
/// [`crate::runs::session::CompletedStack::frame_color`] agree.
pub fn legend_color(accumulated: usize, total: usize) -> [u8; 3] {
    let plane = FramePlane::new(accumulated as u32, total as u32).plane;
    frame_map_color((plane * 65535.0).round() as u16)
}

/// The rail's key during a run, in given order: each frame's
/// [`legend_color`] once taken, else `None`. `taken` counts in accumulation
/// order (last to first when `reversed`). Frames left out are known only
/// from the result, so none is marked.
pub fn running_legend(total: usize, taken: usize, reversed: bool) -> Vec<Option<[u8; 3]>> {
    (0..total)
        .map(|index| {
            let accumulated = if reversed { total - 1 - index } else { index };
            (accumulated < taken).then(|| legend_color(accumulated, total))
        })
        .collect()
}

/// A frame map ([`crate::runs::session::CompletedStack::frame_map`]) as the
/// depth map, in legend colors, from half size down. With `lit`, the
/// frame's share of each pixel ([`PlaneRamp::share`]) keeps its color and
/// the rest dims to gray ([`UNLIT`]).
///
/// Levels pick one pixel per 2×2 block: averaging two frames' planes would
/// name a third frame that supplied nothing there.
pub fn present_frame_map(map: &Gray16Image, lit: Option<&PlaneRamp>) -> PresentedImage {
    let table: Vec<[u8; 4]> = (0..=u16::MAX)
        .into_par_iter()
        .map(|v| {
            let rgb = frame_map_color(v);
            let Some(ramp) = lit else { return display_pixel(rgb) };
            let w = ramp.share(v);
            let [r, g, b] = rgb.map(f32::from);
            let dim = UNLIT * (0.2126 * r + 0.7152 * g + 0.0722 * b);
            display_pixel(rgb.map(|c| (dim + (f32::from(c) - dim) * w).round() as u8))
        })
        .collect();
    let (width, height) = map.dimensions();
    let mut level = pick_half(map.as_raw(), width as usize, height as usize);
    let mut levels = Vec::new();
    loop {
        let (w, h, values) = &level;
        levels.push(tile_bands(*w, *h, |y0, rows| {
            rows.par_chunks_mut(w * 4).zip(values[y0 * w..].par_chunks(*w)).for_each(
                |(dst, src)| {
                    for (d, &v) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                        *d = table[usize::from(v)];
                    }
                },
            );
        }));
        if levels.len() >= MAX_LEVELS - 1 || *w.max(h) <= MIN_LEVEL_SIDE as usize {
            break;
        }
        level = pick_half(values, *w, *h);
    }
    PresentedImage { width, height, first_level: 1, levels, space: ScreenSpace::Srgb }
}

/// A one-channel image at half size, each pixel its 2×2 block's top-left;
/// sized like [`halve`].
fn pick_half(src: &[u16], w: usize, h: usize) -> (usize, usize, Vec<u16>) {
    let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u16; hw * hh];
    out.par_chunks_mut(hw).enumerate().for_each(|(y, dst)| {
        let row = &src[2 * y * w..][..w];
        for (x, d) in dst.iter_mut().enumerate() {
            *d = row[2 * x];
        }
    });
    (hw, hh, out)
}

/// Whether display images convert pixels to the screen's space.
static COLOR_MANAGED: AtomicBool = AtomicBool::new(true);

/// Whether the screen is Display P3 ([`set_screen`]).
static SCREEN_P3: AtomicBool = AtomicBool::new(false);

/// Turns the conversion to the screen's space on or off (on by default),
/// for images presented from then on.
pub fn set_color_managed(on: bool) {
    COLOR_MANAGED.store(on, Ordering::Relaxed);
}

/// Whether the display converts pixels to the screen's space.
pub fn color_managed() -> bool {
    COLOR_MANAGED.load(Ordering::Relaxed)
}

/// The colors of a picture for the screen, which the app tags it with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenSpace {
    /// sRGB, and a standard stack's own values with Color Management off,
    /// shown as they are.
    #[default]
    Srgb,
    DisplayP3,
}

impl ScreenSpace {
    /// The space a draw into `output` is shown in.
    pub fn of(output: OutputSpace) -> Self {
        match output {
            OutputSpace::DisplayP3 => Self::DisplayP3,
            _ => Self::Srgb,
        }
    }
}

/// Sets the space the app's screen shows (sRGB by default): Display P3 on
/// a wide-gamut screen that the app draws in it, for images presented from
/// then on.
pub fn set_screen(space: ScreenSpace) {
    let was = SCREEN_P3.swap(space == ScreenSpace::DisplayP3, Ordering::Relaxed);
    if was != (space == ScreenSpace::DisplayP3) {
        tracing::debug!("the screen is now {space:?}");
    }
}

/// The space the app's screen shows.
pub fn screen() -> ScreenSpace {
    match SCREEN_P3.load(Ordering::Relaxed) {
        true => ScreenSpace::DisplayP3,
        false => ScreenSpace::Srgb,
    }
}

/// The conversion from `profile` to sRGB, or `None` for sRGB or while
/// color management is off: for sRGB pictures such as thumbnails.
pub(crate) fn display_to_srgb(profile: Option<&io::IccProfile>) -> Option<io::RgbConversion> {
    cached(profile.filter(|_| color_managed())?, ScreenSpace::Srgb)
}

/// How stored values become the screen's on the CPU: the conversion, if
/// any, and the space they are then in.
#[derive(Clone, Debug)]
pub struct ToDisplay {
    pub convert: Option<io::RgbConversion>,
    pub space: ScreenSpace,
}

impl ToDisplay {
    /// Values already in sRGB, or shown as they are.
    pub const SRGB: Self = Self { convert: None, space: ScreenSpace::Srgb };

    /// Values the GPU drew into `output`.
    pub fn drawn(output: OutputSpace) -> Self {
        Self { convert: None, space: ScreenSpace::of(output) }
    }
}

/// The CPU conversion a draw of stored values in `profile` (`None`: sRGB)
/// into `output` needs, and the space it lands in.
pub(crate) fn conversion(profile: Option<&io::IccProfile>, output: OutputSpace) -> ToDisplay {
    match output {
        OutputSpace::DisplayP3 => {
            // A profile that can't be built is shown as sRGB is.
            let convert = profile
                .and_then(|p| cached(p, ScreenSpace::DisplayP3))
                .or_else(|| srgb_to_display_p3().clone());
            ToDisplay { convert, space: ScreenSpace::DisplayP3 }
        }
        // `resolve` draws a standard stack's other wide spaces in sRGB.
        OutputSpace::Srgb | OutputSpace::AdobeRgb | OutputSpace::ProPhoto => ToDisplay {
            convert: profile.and_then(|p| cached(p, ScreenSpace::Srgb)),
            space: ScreenSpace::Srgb,
        },
        OutputSpace::StackProfile => ToDisplay::SRGB,
    }
}

/// The screen's space: sRGB or Display P3 while Color Management is on,
/// else a standard image's own, its values shown as they are.
pub(crate) fn display_space() -> OutputSpace {
    match (color_managed(), screen()) {
        (false, _) => OutputSpace::StackProfile,
        (true, ScreenSpace::Srgb) => OutputSpace::Srgb,
        (true, ScreenSpace::DisplayP3) => OutputSpace::DisplayP3,
    }
}

/// `rgb`, shown in `space`, in sRGB: for what is kept or written, which
/// stays sRGB whatever the screen.
pub(crate) fn in_srgb(rgb: image::RgbImage, space: ScreenSpace) -> image::RgbImage {
    static MADE: OnceLock<Option<io::RgbConversion>> = OnceLock::new();
    let convert = match space {
        ScreenSpace::Srgb => return rgb,
        ScreenSpace::DisplayP3 => MADE.get_or_init(io::RgbConversion::display_p3_to_srgb),
    };
    let Some(convert) = convert else { return rgb };
    let (w, h) = rgb.dimensions();
    let wide: Vec<u16> = rgb.as_raw().iter().map(|&v| u16::from(v) * 257).collect();
    let mut out = vec![0; wide.len()];
    convert.convert(&wide, &mut out);
    let narrow = out.into_iter().map(io::u16_to_u8).collect();
    image::RgbImage::from_raw(w, h, narrow).expect("as many values as it had")
}

fn srgb_to_display_p3() -> &'static Option<io::RgbConversion> {
    static MADE: OnceLock<Option<io::RgbConversion>> = OnceLock::new();
    MADE.get_or_init(io::RgbConversion::srgb_to_display_p3)
}

/// The conversion from `profile` into `space`. The last one is cached, as
/// paint asks every stroke.
fn cached(profile: &io::IccProfile, space: ScreenSpace) -> Option<io::RgbConversion> {
    type Last = Option<(io::IccProfile, ScreenSpace, Option<io::RgbConversion>)>;
    static LAST: Mutex<Last> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, s, t)) = last.as_ref()
        && p == profile
        && *s == space
    {
        return t.clone();
    }
    let built = match space {
        ScreenSpace::Srgb => profile.to_srgb(),
        ScreenSpace::DisplayP3 => profile.to_display_p3(),
    };
    if built.is_none() {
        tracing::warn!(
            "{} can't be converted for display; shown as sRGB",
            profile.name().as_deref().unwrap_or("an unnamed profile")
        );
    }
    *last = Some((profile.clone(), space, built.clone()));
    built
}

/// Display pixels for interleaved RGB u16 `src`, through `to_srgb` if any.
pub(crate) fn display_rgb16(src: &[u16], dst: &mut [u8], to_srgb: Option<&io::RgbConversion>) {
    let converted;
    let src = match to_srgb {
        Some(t) => {
            let mut out = vec![0; src.len()];
            t.convert(src, &mut out);
            converted = out;
            &converted
        }
        None => src,
    };
    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<3>().0) {
        *d = display_pixel(s.map(io::u16_to_u8));
    }
}

/// [`present_rgb`] for u16 pixels, through `to` ([`conversion`]).
pub(crate) fn present_rgb16(
    pixels: &[u16],
    width: u32,
    height: u32,
    to: &ToDisplay,
) -> PresentedImage {
    let to_screen = to.convert.as_ref();
    present_rows(pixels, width, height, |src, dst| display_rgb16(src, dst, to_screen))
        .in_space(to.space)
}

/// Where red, green and blue sit in a [`DISPLAY_FORMAT`] pixel.
const DISPLAY_RGB: [usize; 3] = {
    assert!(matches!(DISPLAY_FORMAT, wgpu::TextureFormat::Bgra8Unorm));
    [2, 1, 0]
};

/// An opaque display pixel, in [`DISPLAY_FORMAT`]'s channel order.
pub(crate) fn display_pixel(rgb: [u8; 3]) -> [u8; 4] {
    let mut pixel = [255; 4];
    for (c, i) in DISPLAY_RGB.into_iter().enumerate() {
        pixel[i] = rgb[c];
    }
    pixel
}

/// A display pixel's red, green and blue: [`display_pixel`] undone.
fn rgb_of_display(pixel: [u8; 4]) -> [u8; 3] {
    DISPLAY_RGB.map(|i| pixel[i])
}

/// Interleaved RGB pixels, rounded to 8 bits by `to_u8`, as displayed.
pub fn present_rgb<T: Copy + Sync>(
    pixels: &[T],
    width: u32,
    height: u32,
    to_u8: impl Fn(T) -> u8 + Sync,
) -> PresentedImage {
    present_rows(pixels, width, height, |src, dst| {
        for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<3>().0) {
            *d = display_pixel(s.map(&to_u8));
        }
    })
}

/// Interleaved RGB `pixels` as displayed, each row made by `row`, a band
/// at a time across threads.
fn present_rows<T: Sync>(
    pixels: &[T],
    width: u32,
    height: u32,
    row: impl Fn(&[T], &mut [u8]) + Sync,
) -> PresentedImage {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(pixels.len(), w * h * 3, "{width}x{height} RGB");
    let mut levels = Levels::new(w, h, MAX_LEVELS);
    for src in pixels.chunks(BAND_ROWS * w * 3) {
        levels.fill(src.len() / (w * 3), |dst| {
            dst.par_chunks_mut(w * 4).zip(src.par_chunks(w * 3)).for_each(|(d, s)| row(s, d));
        });
    }
    levels.finish()
}

/// A rendered Rgba16Unorm or Rgba32Float texture as displayed, read back
/// in bands and converted through `to` ([`conversion`]) straight from the
/// mapping.
pub fn present_texture(
    ctx: &GpuContext,
    texture: &wgpu::Texture,
    to: &ToDisplay,
) -> Result<PresentedImage> {
    let to_srgb = to.convert.as_ref();
    let source = texture.format();
    let texel = match source {
        wgpu::TextureFormat::Rgba16Unorm => 8,
        wgpu::TextureFormat::Rgba32Float => 16,
        other => {
            return Err(Error::InvalidState(format!("cannot display a {other:?} texture")));
        }
    };
    let (w, h) = (texture.width() as usize, texture.height() as usize);
    let mut levels = Levels::new(w, h, MAX_LEVELS);
    download_texture_rows(
        ctx,
        texture,
        [0, 0],
        [w as u32, h as u32],
        |_, mapped, stride, bytes| {
            levels.fill(mapped.len() / stride, |dst| {
                dst.par_chunks_mut(w * 4).zip(mapped.par_chunks(stride)).for_each(|(d, s)| {
                    let rgb: Vec<u16> =
                        s[..bytes].chunks_exact(texel).flat_map(source_rgb).collect();
                    display_rgb16(&rgb, d, to_srgb);
                });
            });
        },
    )
    .map_err(|e| Error::Core(e.into()))?;
    Ok(levels.finish().in_space(to.space))
}

/// The RGB u16 codes of one Rgba16Unorm (8-byte) or Rgba32Float (16-byte)
/// texel, floats rounded by [`io::quantize_u16`].
pub(crate) fn source_rgb(texel: &[u8]) -> [u16; 3] {
    let channel = |c: usize| match texel.len() {
        8 => u16::from_le_bytes([texel[2 * c], texel[2 * c + 1]]),
        _ => io::quantize_u16(f32::from_le_bytes(texel[4 * c..][..4].try_into().unwrap())),
    };
    [channel(0), channel(1), channel(2)]
}

/// The live map (`Stacker::render_live`, 8-bit planes) in the depth map's
/// colors: one level, already reduced by the stacker.
pub fn present_map(map: &GrayImage) -> CpuImage {
    let (width, height) = map.dimensions();
    color(map.as_raw(), width, height, &plasma_table())
}

/// Gray values through a 256-entry display table.
fn color(gray: &[u8], width: u32, height: u32, table: &[[u8; 4]; 256]) -> CpuImage {
    let mut pixels = Vec::with_capacity(gray.len() * 4);
    pixels.extend(gray.iter().flat_map(|&v| table[usize::from(v)]));
    CpuImage { width, height, pixels }
}

/// Packed 8-bit pixels, one channel or four (each user's `C`).
struct Plane {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

/// A level whole, cut into gutter-padded tiles (see [`TiledLevel`]).
fn tile(plane: &Plane) -> TiledLevel {
    let row = plane.width * 4;
    tile_bands(plane.width, plane.height, |y0, rows| {
        rows.copy_from_slice(&plane.pixels[y0 * row..][..rows.len()]);
    })
}

/// A level cut into tiles, its 4-channel rows made by `fill(first_row,
/// rows)` a band at a time, so it is never held whole.
fn tile_bands(width: usize, height: usize, fill: impl Fn(usize, &mut [u8])) -> TiledLevel {
    let mut levels = Levels::new(width, height, 1);
    for y0 in (0..height).step_by(BAND_ROWS) {
        levels.fill(BAND_ROWS.min(height - y0), |rows| fill(y0, rows));
    }
    levels.finish().levels.remove(0)
}

/// Rows of a picture made and handed to [`Levels`] at once: two tile rows.
const BAND_ROWS: usize = 2 * TILE_CORE as usize;

/// A display image's levels made a band at a time: each band is tiled and
/// halved into the next level, so no level is held whole. The tiles match
/// those of whole levels byte for byte.
struct Levels {
    levels: Vec<Strip>,
}

impl Levels {
    /// At most `count` levels, ending at the first within [`MIN_LEVEL_SIDE`].
    fn new(width: usize, height: usize, count: usize) -> Self {
        let mut levels = vec![Strip::new(width, height)];
        while let Some(last) = levels.last()
            && levels.len() < count
            && last.width.max(last.height) > MIN_LEVEL_SIDE as usize
        {
            levels.push(Strip::new(last.width.div_ceil(2), last.height.div_ceil(2)));
        }
        Self { levels }
    }

    /// Hands over the first level's next `rows` rows, filled by `fill`.
    fn fill(&mut self, rows: usize, fill: impl FnOnce(&mut [u8])) {
        let first = &mut self.levels[0];
        let at = first.rows.len();
        first.grow(rows);
        fill(&mut first.rows[at..]);
        Self::taken(&mut self.levels, rows);
    }

    /// Tiles what `rows` more complete and halves them down the chain.
    fn taken(levels: &mut [Strip], rows: usize) {
        let Some((level, below)) = levels.split_first_mut() else { return };
        level.take(rows);
        match below.first_mut() {
            Some(next) => {
                let made = level.halve_into(next);
                if made > 0 {
                    Self::taken(below, made);
                }
            }
            // The last level halves into nothing: no row is kept for it.
            None => level.halved = level.received,
        }
        level.trim();
    }

    /// The image, every row handed over.
    fn finish(self) -> PresentedImage {
        let (width, height) = (self.levels[0].width as u32, self.levels[0].height as u32);
        let levels = self.levels.into_iter().map(Strip::finish).collect();
        PresentedImage { width, height, first_level: 0, levels, space: ScreenSpace::Srgb }
    }
}

/// One level of [`Levels`] as it is made.
struct Strip {
    width: usize,
    height: usize,
    /// The rows kept, from row `first`.
    rows: Vec<u8>,
    first: usize,
    /// Rows handed over so far.
    received: usize,
    /// Rows of the next level made from them; for the last level, every
    /// row received.
    halved: usize,
    /// The tiles cut, whole tile rows, row-major.
    tiles: Vec<CpuImage>,
}

impl Strip {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            rows: Vec::new(),
            first: 0,
            received: 0,
            halved: 0,
            tiles: Vec::new(),
        }
    }

    fn cols(&self) -> usize {
        self.width.div_ceil(TILE_CORE as usize)
    }

    /// Room for `rows` more, zeroed. Exactly that much, so the strip settles
    /// at its peak rather than doubling past it.
    fn grow(&mut self, rows: usize) {
        let bytes = rows * self.width * 4;
        self.rows.reserve_exact(bytes);
        self.rows.resize(self.rows.len() + bytes, 0);
    }

    /// Row `y`, which must be kept.
    fn row(&self, y: usize) -> &[u8] {
        &self.rows[(y - self.first) * self.width * 4..][..self.width * 4]
    }

    /// Cuts every tile row `rows` more complete (including the gutter row
    /// below the core).
    fn take(&mut self, rows: usize) {
        self.received += rows;
        let core = TILE_CORE as usize;
        while self.tiles.len() < self.cols() * self.height.div_ceil(core) {
            let ty = self.tiles.len() / self.cols();
            if self.received <= ((ty + 1) * core).min(self.height - 1) {
                break;
            }
            let tiles: Vec<CpuImage> =
                (0..self.cols()).into_par_iter().map(|tx| self.cut(tx, ty)).collect();
            self.tiles.extend(tiles);
        }
    }

    /// Tile `(tx, ty)`, its gutter repeating the nearest pixel inside.
    fn cut(&self, tx: usize, ty: usize) -> CpuImage {
        let (w, h, core) = (self.width, self.height, TILE_CORE as usize);
        let (x0, y0) = (tx * core, ty * core);
        let (tw, th) = ((w - x0).min(core) + 2, (h - y0).min(core) + 2);
        let mut pixels = Vec::with_capacity(tw * th * 4);
        for y in 0..th {
            let row = self.row((y0 + y).saturating_sub(1).min(h - 1));
            let (first, last) = (x0.saturating_sub(1), (x0 + tw - 2).min(w - 1));
            if x0 == 0 {
                pixels.extend_from_slice(&row[..4]);
            }
            pixels.extend_from_slice(&row[first * 4..(last + 1) * 4]);
            if x0 + tw - 2 == w {
                pixels.extend_from_slice(&row[(w - 1) * 4..]);
            }
        }
        debug_assert_eq!(pixels.len(), tw * th * 4);
        CpuImage { width: tw as u32, height: th as u32, pixels }
    }

    /// Halves the newly available rows into `next` (an odd last row or
    /// column averages with itself); returns how many.
    fn halve_into(&mut self, next: &mut Strip) -> usize {
        let ready = if self.received == self.height { next.height } else { self.received / 2 };
        let (from, w, h) = (self.halved, self.width, self.height);
        let at = next.rows.len();
        next.grow(ready - from);
        next.rows[at..].par_chunks_mut(next.width * 4).enumerate().for_each(|(i, dst)| {
            let y = from + i;
            let (r0, r1) = (self.row(2 * y), self.row((2 * y + 1).min(h - 1)));
            for (x, d) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                average(r0, r1, 2 * x * 4, (2 * x + 1).min(w - 1) * 4, d);
            }
        });
        self.halved = ready;
        ready - from
    }

    /// Drops rows above the next tile row's gutter and the next halving.
    fn trim(&mut self) {
        let core = TILE_CORE as usize;
        let tile_rows = self.tiles.len() / self.cols();
        let tiles_need = if tile_rows == self.height.div_ceil(core) {
            self.received
        } else {
            (tile_rows * core).saturating_sub(1)
        };
        let keep = tiles_need.min(2 * self.halved).min(self.received).max(self.first);
        self.rows.drain(..(keep - self.first) * self.width * 4);
        self.first = keep;
    }

    fn finish(self) -> TiledLevel {
        debug_assert_eq!(self.received, self.height, "every row handed over");
        let (cols, rows) = (self.cols(), self.height.div_ceil(TILE_CORE as usize));
        TiledLevel {
            width: self.width as u32,
            height: self.height as u32,
            cols: cols as u32,
            rows: rows as u32,
            tiles: self.tiles,
        }
    }
}

/// The tiles whose stored pixels (gutter included) touch `rect`.
pub(crate) fn tiles_touching(level: [u32; 2], rect: PixelRect) -> [std::ops::Range<u32>; 2] {
    let span = |from: u32, to: u32, len: u32| {
        let count = len.div_ceil(TILE_CORE);
        (from.saturating_sub(1) / TILE_CORE).min(count)..(to / TILE_CORE + 1).min(count)
    };
    [span(rect.x, rect.right(), level[0]), span(rect.y, rect.bottom(), level[1])]
}

/// The level pixels tile `(tx, ty)` stores, gutter included, clamped to
/// the level.
pub(crate) fn stored_rect(level: [u32; 2], tx: u32, ty: u32) -> PixelRect {
    let side = |t: u32, len: u32| {
        let from = (t * TILE_CORE).saturating_sub(1);
        (from, ((t + 1) * TILE_CORE + 1).min(len) - from)
    };
    let ((x, width), (y, height)) = (side(tx, level[0]), side(ty, level[1]));
    PixelRect { x, y, width, height }
}

/// The level pixels tile `(tx, ty)` covers, its core: no gutter.
pub(crate) fn core_rect(level: [u32; 2], tx: u32, ty: u32) -> PixelRect {
    let (x, y) = (tx * TILE_CORE, ty * TILE_CORE);
    let (width, height) = ((level[0] - x).min(TILE_CORE), (level[1] - y).min(TILE_CORE));
    PixelRect { x, y, width, height }
}

/// Tile `(tx, ty)` cut from `src`, a patch at `at` holding the tile's
/// [`stored_rect`]; the same pixels as tiling the whole level.
pub(crate) fn cut_tile(
    src: &CpuImage,
    at: [u32; 2],
    level: [u32; 2],
    tx: u32,
    ty: u32,
) -> CpuImage {
    let (x0, y0) = (tx * TILE_CORE, ty * TILE_CORE);
    let tw = (level[0] - x0).min(TILE_CORE) + 2;
    let th = (level[1] - y0).min(TILE_CORE) + 2;
    let sw = src.width as usize;
    let mut pixels = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        let sy = ((y0 + y).saturating_sub(1).min(level[1] - 1) - at[1]) as usize;
        let row = &src.pixels[sy * sw * 4..][..sw * 4];
        for x in 0..tw {
            let sx = ((x0 + x).saturating_sub(1).min(level[0] - 1) - at[0]) as usize;
            pixels.extend_from_slice(&row[sx * 4..sx * 4 + 4]);
        }
    }
    CpuImage { width: tw, height: th, pixels }
}

/// One halved pixel: the rounded mean of columns `x0` and `x1` (byte
/// offsets) of rows `r0` and `r1`.
#[inline]
fn average<const C: usize>(r0: &[u8], r1: &[u8], x0: usize, x1: usize, d: &mut [u8; C]) {
    for c in 0..C {
        let sum = u16::from(r0[x0 + c])
            + u16::from(r0[x1 + c])
            + u16::from(r1[x0 + c])
            + u16::from(r1[x1 + c]);
        d[c] = ((sum + 2) / 4) as u8;
    }
}

/// A rectangle of pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    pub fn right(self) -> u32 {
        self.x + self.width
    }

    pub fn bottom(self) -> u32 {
        self.y + self.height
    }

    /// The smallest rect holding both.
    pub(crate) fn union(self, other: Self) -> Self {
        let (x, y) = (self.x.min(other.x), self.y.min(other.y));
        let (right, bottom) = (self.right().max(other.right()), self.bottom().max(other.bottom()));
        Self { x, y, width: right - x, height: bottom - y }
    }

    /// The pixels in both, if any.
    pub(crate) fn intersect(self, other: Self) -> Option<Self> {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let (right, bottom) = (self.right().min(other.right()), self.bottom().min(other.bottom()));
        (x < right && y < bottom).then(|| Self { x, y, width: right - x, height: bottom - y })
    }

    /// Grown to multiples of `RECT_ALIGN` within the level, so every level
    /// it halves into stays aligned and [`present_rect`] needs nothing
    /// outside it.
    pub fn aligned(self, width: u32, height: u32) -> Self {
        let (x, y) = (self.x / RECT_ALIGN * RECT_ALIGN, self.y / RECT_ALIGN * RECT_ALIGN);
        let right = self.right().next_multiple_of(RECT_ALIGN).min(width);
        let bottom = self.bottom().next_multiple_of(RECT_ALIGN).min(height);
        Self { x, y, width: right - x, height: bottom - y }
    }

    /// The pixels of the next level down that this rect's pixels average
    /// into.
    pub fn halved(self) -> Self {
        let (x, y) = (self.x / 2, self.y / 2);
        Self { x, y, width: self.right().div_ceil(2) - x, height: self.bottom().div_ceil(2) - y }
    }
}

/// What [`PixelRect::aligned`] rounds to: even down to the last level.
const RECT_ALIGN: u32 = 1 << (MAX_LEVELS - 1);

/// Recomputes `rect` of `dst`, the halving of a 4-channel level, from
/// `src`, a patch at `at` holding every pixel the rect averages (an
/// [`PixelRect::aligned`] rect or the whole level).
pub(crate) fn present_rect(
    src: &CpuImage,
    at: [u32; 2],
    level_size: [u32; 2],
    dst: &mut CpuImage,
    rect: PixelRect,
) {
    let [w, h] = level_size.map(|v| v as usize);
    let (sw, ax, ay) = (src.width as usize, at[0] as usize, at[1] as usize);
    assert!(
        2 * rect.x as usize >= ax
            && 2 * rect.y as usize >= ay
            && (2 * rect.right() as usize).min(w) <= ax + sw
            && (2 * rect.bottom() as usize).min(h) <= ay + src.height as usize,
        "{rect:?} averages pixels outside the {}x{} patch at {at:?}",
        src.width,
        src.height
    );
    assert!(rect.right() <= dst.width && rect.bottom() <= dst.height, "{rect:?} outside dst");
    let row = |y: usize| &src.pixels[(y - ay) * sw * 4..][..sw * 4];
    let dw = dst.width as usize;
    let (x0, x1) = (rect.x as usize, rect.right() as usize);
    dst.pixels
        .par_chunks_mut(dw * 4)
        .skip(rect.y as usize)
        .take(rect.height as usize)
        .enumerate()
        .for_each(|(i, out)| {
            let y = rect.y as usize + i;
            let (r0, r1) = (row(2 * y), row((2 * y + 1).min(h - 1)));
            for (x, d) in out.as_chunks_mut::<4>().0[x0..x1].iter_mut().enumerate() {
                let x = x0 + x;
                average(r0, r1, (2 * x - ax) * 4, ((2 * x + 1).min(w - 1) - ax) * 4, d);
            }
        });
}

/// A level whose longest side is this or less gets no further halving.
const MIN_LEVEL_SIDE: u32 = 1024;
/// Cap on the chain length (full + 3 halvings covers 8 K down to 1 K).
const MAX_LEVELS: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fit_rounds_to_the_nearest_pixel() {
        assert_eq!(fit_long_edge(8192, 5464, 3840), (3840, 2561));
        assert_eq!(fit_long_edge(5464, 8192, 3840), (2561, 3840));
        assert_eq!(fit_long_edge(100, 50, 400), (100, 50), "never up");
        assert_eq!(fit_long_edge(100, 50, 0), (100, 50));
        assert_eq!(fit_long_edge(10_000, 1, 100), (100, 1), "at least a pixel");
        // 351 × 160 / 2496 is 22.5 exactly, which a float made 22.
        assert_eq!(fit_long_edge(2496, 351, 160), (160, 23));
    }

    #[test]
    fn rgb_goes_to_the_display_and_back() {
        let rgb = image::RgbImage::from_raw(2, 1, vec![10, 20, 30, 200, 100, 0]).unwrap();
        let shown = CpuImage::from_rgb(&rgb);
        assert_eq!(shown.pixels, [30, 20, 10, 255, 0, 100, 200, 255], "BGRA, opaque");
        assert_eq!(shown.to_rgb(), rgb);
    }

    #[test]
    fn a_running_stack_colors_each_frame_taken_as_its_map_will() {
        // Nothing taken yet: every tick waits.
        assert_eq!(running_legend(4, 0, false), vec![None; 4]);
        // Two of four taken, in the order given: the first two have their
        // colors, the first frame's the start of the ramp.
        let two = running_legend(4, 2, false);
        assert_eq!(two[0], Some(PLASMA[0]));
        assert_eq!(two[1], Some(legend_color(1, 4)));
        assert_eq!(two[2..], [None, None]);
        // Reversed, the last frame given is taken first, in the first color.
        let back = running_legend(4, 1, true);
        assert_eq!(back, vec![None, None, None, Some(PLASMA[0])]);
        // All taken: each frame's color is the finished map's, the last
        // frame the end of the ramp.
        let all = running_legend(4, 4, false);
        assert_eq!(all[3], Some(PLASMA[255]));
        assert!(all.iter().all(Option::is_some));
    }

    #[test]
    fn a_frame_map_shows_each_frame_in_its_color_and_lights_one() {
        // Two frames side by side: plane 0 on the left, plane 1 on the right.
        let map =
            Gray16Image::from_fn(8, 4, |x, _| image::Luma([if x < 4 { 0 } else { u16::MAX }]));
        let shown = present_frame_map(&map, None);
        assert_eq!((shown.width(), shown.height(), shown.first_level), (8, 4, 1));
        let level = shown.levels[0].assemble();
        assert_eq!((level.width, level.height), (4, 2));
        assert_eq!(level.pixels[..4], display_pixel(PLASMA[0]));
        assert_eq!(level.pixels[12..16], display_pixel(PLASMA[255]));
        assert_eq!(frame_map_color(0), PLASMA[0]);
        assert_eq!(frame_map_color(u16::MAX), PLASMA[255]);

        // Lit for the last frame: its side keeps the color, the other dims
        // to gray.
        let last = PlaneRamp::new(1, 2, |_| true);
        let lit = present_frame_map(&map, Some(&last)).levels[0].assemble();
        assert_eq!(lit.pixels[12..16], display_pixel(PLASMA[255]));
        let [b, g, r, _] = lit.pixels[..4] else { unreachable!() };
        assert!(b == g && g == r && r < 20, "unlit is dim gray: {r} {g} {b}");
    }

    #[test]
    fn a_frame_map_level_never_names_a_frame_between_two() {
        // A checkerboard of the first and last frames: an average would
        // show the middle of the legend.
        let map = Gray16Image::from_fn(4096, 4, |x, y| {
            image::Luma([if (x + y) % 2 == 0 { 0 } else { u16::MAX }])
        });
        let ends = [display_pixel(PLASMA[0]), display_pixel(PLASMA[255])];
        let shown = present_frame_map(&map, None);
        assert!(shown.levels.len() > 1);
        for level in &shown.levels {
            let image = level.assemble();
            assert!(image.pixels.as_chunks::<4>().0.iter().all(|p| ends.contains(p)));
        }
    }

    #[test]
    fn a_tile_cut_from_a_patch_matches_the_tiling() {
        let (w, h) = (1100usize, 1030usize);
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i * 31 % 251) as u8).collect();
        let plane = Plane { width: w, height: h, pixels: pixels.clone() };
        let tiled = tile(&plane);
        let level = [w as u32, h as u32];
        let whole = CpuImage { width: w as u32, height: h as u32, pixels };
        for ty in 0..tiled.rows {
            for tx in 0..tiled.cols {
                let r = stored_rect(level, tx, ty);
                let patch = CpuImage {
                    width: r.width,
                    height: r.height,
                    pixels: (r.y..r.bottom())
                        .flat_map(|y| {
                            &whole.pixels[(y as usize * w + r.x as usize) * 4..]
                                [..r.width as usize * 4]
                        })
                        .copied()
                        .collect(),
                };
                let cut = cut_tile(&patch, [r.x, r.y], level, tx, ty);
                let want = &tiled.tiles[(ty * tiled.cols + tx) as usize];
                assert!(cut.pixels == want.pixels && cut.width == want.width, "tile {tx},{ty}");
            }
        }
        // Pixel 510 is tile 1's first core pixel and tile 0's right gutter.
        let one = PixelRect { x: 510, y: 0, width: 1, height: 1 };
        assert_eq!(tiles_touching(level, one), [0..2, 0..1]);
        let inside = PixelRect { x: 600, y: 600, width: 10, height: 10 };
        assert_eq!(tiles_touching(level, inside), [1..2, 1..2]);
        let edge = PixelRect { x: 1099, y: 1029, width: 1, height: 1 };
        assert_eq!(tiles_touching(level, edge), [2..3, 2..3]);
    }

    #[test]
    fn halving_covers_odd_edges_and_rounds() {
        // 3×3: the odd last row/column average with themselves.
        let src = [0, 4, 9, 8, 0, 9, 100, 100, 50];
        let half = halve::<1>(&src, 3, 3);
        assert_eq!((half.width, half.height), (2, 2));
        assert_eq!(half.pixels, [3, 9, 100, 50]);
        // Four channels average independently: the same values in channel
        // 1, their complement in channel 2, opaque alpha stays opaque.
        let rgba: Vec<u8> = src.iter().flat_map(|&v| [v, v, 255 - v, 255]).collect();
        let half = halve::<4>(&rgba, 3, 3);
        let expect: Vec<u8> =
            [3u8, 9, 100, 50].iter().flat_map(|&v| [v, v, 255 - v, 255]).collect();
        assert_eq!(half.pixels, expect);
    }

    #[test]
    fn rgb_levels_stop_at_the_minimum_side() {
        // 16-bit RGB at 2049×65: halving rounds up, so 1025 is still above
        // the 1024 px floor and one more level follows.
        let (w, h) = (2049u32, 65u32);
        let pixels: Vec<u16> = (0..w * h * 3).map(|i| (i % 3) as u16 * 0x7fff).collect();
        let shown = present_rgb(&pixels, w, h, io::u16_to_u8);
        assert_eq!((shown.width(), shown.height(), shown.first_level), (w, h, 0));
        let sizes: Vec<_> = shown.levels.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, [(2049, 65), (1025, 33), (513, 17)]);
        // R 0, G 0x7fff → 127, B 0xfffe → 255, in BGRA order.
        assert_eq!(&shown.levels[0].assemble().pixels[..4], &[255, 127, 0, 255]);
        assert_eq!(&shown.levels[1].assemble().pixels[..4], &[255, 127, 0, 255]);
    }

    #[test]
    fn texels_round_like_the_result() {
        let unorm: Vec<u8> =
            [1u16, 0x8000, 0xffff, 7].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(source_rgb(&unorm), [1, 0x8000, 0xffff]);
        let float: Vec<u8> =
            [0.5f32, -1.0, 2.0, 0.0].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(source_rgb(&float), [io::quantize_u16(0.5), 0, 0xffff]);
    }

    #[test]
    fn a_map_is_plasma_in_the_display_channel_order() {
        let map = GrayImage::from_raw(1, 1, vec![100]).unwrap();
        let [r, g, b] = PLASMA[100];
        assert_eq!(present_map(&map).pixels, [b, g, r, 255]);
        // Ends of the scale: first frame dark purple, last frame yellow.
        assert_eq!(PLASMA[0], [13, 8, 135]);
        assert_eq!(PLASMA[255], [240, 249, 33]);
    }

    #[test]
    fn the_live_picture_is_in_the_depth_map_s_colors() {
        let planes = GrayImage::from_raw(2, 1, vec![0, 255]).unwrap();
        let live = present_map(&planes);
        let ([r0, g0, b0], [r1, g1, b1]) = (PLASMA[0], PLASMA[255]);
        assert_eq!((live.width, live.height), (2, 1));
        assert_eq!(live.pixels, [b0, g0, r0, 255, b1, g1, r1, 255]);
    }

    /// `first` followed by its halvings: `count` levels at most, ending at the
    /// first whose longest side is within [`MIN_LEVEL_SIDE`].
    fn halvings<const C: usize>(first: Plane, count: usize) -> Vec<Plane> {
        let mut levels = vec![first];
        while let Some(last) = levels.last()
            && levels.len() < count
            && last.width.max(last.height) > MIN_LEVEL_SIDE as usize
        {
            levels.push(halve::<C>(&last.pixels, last.width, last.height));
        }
        levels
    }

    /// 2×2 box average of a `w`×`h` image of `C` channels, rounded; an odd last
    /// row or column averages with itself, so the half image covers the whole
    /// source.
    fn halve<const C: usize>(src: &[u8], w: usize, h: usize) -> Plane {
        let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
        let mut out = vec![0u8; hw * hh * C];
        out.par_chunks_mut(hw * C).enumerate().for_each(|(y, dst)| {
            let r0 = &src[2 * y * w * C..][..w * C];
            let r1 = &src[(2 * y + 1).min(h - 1) * w * C..][..w * C];
            for (x, d) in dst.as_chunks_mut::<C>().0.iter_mut().enumerate() {
                average(r0, r1, 2 * x * C, (2 * x + 1).min(w - 1) * C, d);
            }
        });
        Plane { width: hw, height: hh, pixels: out }
    }

    /// Cut a 4-channel plane into gutter-padded tiles the whole level at
    /// once: what [`Levels`]' tiles are checked against.
    fn tile_whole(plane: &Plane) -> TiledLevel {
        let (w, h) = (plane.width, plane.height);
        let core = TILE_CORE as usize;
        let (cols, rows) = (w.div_ceil(core), h.div_ceil(core));
        let tiles = (0..cols * rows)
            .map(|i| {
                let (x0, y0) = ((i % cols) * core, (i / cols) * core);
                let (tw, th) = ((w - x0).min(core) + 2, (h - y0).min(core) + 2);
                let mut pixels = Vec::with_capacity(tw * th * 4);
                for y in 0..th {
                    let sy = (y0 + y).saturating_sub(1).min(h - 1);
                    for x in 0..tw {
                        let sx = (x0 + x).saturating_sub(1).min(w - 1);
                        pixels.extend_from_slice(&plane.pixels[(sy * w + sx) * 4..][..4]);
                    }
                }
                CpuImage { width: tw as u32, height: th as u32, pixels }
            })
            .collect();
        TiledLevel {
            width: w as u32,
            height: h as u32,
            cols: cols as u32,
            rows: rows as u32,
            tiles,
        }
    }

    #[test]
    fn levels_made_by_bands_are_the_whole_levels_tiled() {
        // Odd sizes at every level, a chain cut short by its length and one
        // by its side, bands of every awkward size, and the band rows.
        for (w, h) in [(1, 1), (3, 2), (1025, 2049), (4111, 2053), (2100, 1531), (5000, 300)] {
            let plane = numbered(w, h);
            let whole: Vec<TiledLevel> =
                halvings::<4>(Plane { pixels: plane.pixels.clone(), ..plane }, MAX_LEVELS)
                    .iter()
                    .map(tile_whole)
                    .collect();
            for band in [1, 7, 509, 510, 511, BAND_ROWS, 3001] {
                let mut levels = Levels::new(w, h, MAX_LEVELS);
                for rows in plane.pixels.chunks(band * w * 4) {
                    levels.fill(rows.len() / (w * 4), |to| to.copy_from_slice(rows));
                    // Never a level whole: at most a band and two tile rows.
                    let held: usize = levels.levels.iter().map(|l| l.rows.len()).sum();
                    assert!(held <= (band + 2 * TILE_CORE as usize + 2) * w * 4 * 2, "{held}");
                }
                let made = levels.finish();
                assert_eq!(made.levels.len(), whole.len(), "{w}x{h}");
                for (k, (got, want)) in made.levels.iter().zip(&whole).enumerate() {
                    assert_eq!((got.cols, got.rows), (want.cols, want.rows));
                    assert!(
                        got.tiles.iter().zip(&want.tiles).all(|(a, b)| a.pixels == b.pixels
                            && (a.width, a.height) == (b.width, b.height)),
                        "{w}x{h} level {k}, bands of {band}"
                    );
                }
            }
        }
    }

    /// The last level, which halves into nothing, lets go of its rows as
    /// its tiles are cut, as the levels above it do: a level tiled alone
    /// (the depth map's, a single picture's) never holds itself whole.
    #[test]
    fn the_last_level_s_rows_are_trimmed() {
        let (w, h) = (700, 2100);
        let plane = numbered(w, h);
        for count in [1, 2] {
            let mut levels = Levels::new(w, h, count);
            for rows in plane.pixels.chunks(BAND_ROWS * w * 4) {
                levels.fill(rows.len() / (w * 4), |to| to.copy_from_slice(rows));
                let last = levels.levels.last().unwrap();
                let most = BAND_ROWS / (1 << (count - 1)) + TILE_CORE as usize + 2;
                assert!(
                    last.rows.len() <= most * last.width * 4,
                    "{count} levels: {}",
                    last.rows.len()
                );
            }
            let made = levels.finish();
            assert_eq!(made.levels[0].assemble().pixels, plane.pixels);
        }
        assert_eq!(tile(&plane).assemble().pixels, plane.pixels, "tiled in bands");
    }

    /// A `w`×`h` BGRA plane whose every pixel is distinct.
    fn numbered(w: usize, h: usize) -> Plane {
        let pixels = (0..w * h).flat_map(|i| [i as u8, (i >> 8) as u8, (i >> 16) as u8, 255]);
        Plane { width: w, height: h, pixels: pixels.collect() }
    }

    #[test]
    fn tiles_reassemble_to_the_level_byte_for_byte() {
        let core = TILE_CORE as usize;
        for (w, h) in [(1, 1), (core, core), (core + 1, 2 * core - 1), (3 * core + 7, 5)] {
            let plane = numbered(w, h);
            let level = tile(&plane);
            assert_eq!(
                (level.cols, level.rows),
                (w.div_ceil(core) as u32, h.div_ceil(core) as u32)
            );
            let again = level.assemble();
            assert_eq!((again.width, again.height), (w as u32, h as u32));
            assert!(again.pixels == plane.pixels, "{w}x{h}");
        }
    }

    #[test]
    fn gutters_repeat_the_neighboring_pixels() {
        // 3 tiles across, 2 down: every gutter pixel is the level pixel
        // just outside its core, clamped into the image at the border.
        let core = TILE_CORE as usize;
        let (w, h) = (2 * core + 9, core + 4);
        let plane = numbered(w, h);
        let level = tile(&plane);
        let at = |x: usize, y: usize| &plane.pixels[(y * w + x) * 4..][..4];
        for (i, t) in level.tiles.iter().enumerate() {
            let (x0, y0) = ((i % 3) * core, (i / 3) * core);
            let tw = t.width as usize;
            assert!(tw <= TILE_CORE as usize + 2 && t.height <= TILE_CORE + 2);
            for y in 0..t.height as usize {
                for x in 0..tw {
                    let sx = (x0 + x).saturating_sub(1).min(w - 1);
                    let sy = (y0 + y).saturating_sub(1).min(h - 1);
                    assert_eq!(
                        &t.pixels[(y * tw + x) * 4..][..4],
                        at(sx, sy),
                        "tile {i} ({x}, {y})"
                    );
                }
            }
        }
    }

    fn crop(plane: &Plane, r: PixelRect) -> CpuImage {
        let (x, w) = (r.x as usize * 4, r.width as usize * 4);
        let rows =
            (r.y..r.bottom()).map(|y| &plane.pixels[y as usize * plane.width * 4..][x..x + w]);
        CpuImage { width: r.width, height: r.height, pixels: rows.flatten().copied().collect() }
    }

    fn as_image(plane: &Plane) -> CpuImage {
        let (width, height) = (plane.width as u32, plane.height as u32);
        CpuImage { width, height, pixels: plane.pixels.clone() }
    }

    #[test]
    fn a_patched_level_equals_a_full_re_present() {
        // Odd sizes at every level, and rects inside, at the corner and
        // across the odd right and bottom edges.
        let (w, h) = (8301usize, 4103usize);
        let base = numbered(w, h);
        for dirty in [
            PixelRect { x: 333, y: 101, width: 444, height: 455 },
            PixelRect { x: 0, y: 0, width: 1, height: 1 },
            PixelRect { x: 8000, y: 4000, width: 301, height: 103 },
        ] {
            let before = halvings::<4>(Plane { pixels: base.pixels.clone(), ..base }, MAX_LEVELS);
            assert_eq!(before.len(), MAX_LEVELS);
            let mut painted = Plane { pixels: base.pixels.clone(), ..base };
            for y in dirty.y..dirty.bottom() {
                for x in dirty.x..dirty.right() {
                    let i = (y as usize * w + x as usize) * 4;
                    painted.pixels[i..i + 3].copy_from_slice(&[(x * 7) as u8, (y * 3) as u8, 9]);
                }
            }
            let expect =
                halvings::<4>(Plane { pixels: painted.pixels.clone(), ..painted }, MAX_LEVELS);

            // Level 0 comes as a patch of the aligned rect (paint renders
            // it); the levels below are kept whole and patched in place.
            let mut rect = dirty.aligned(w as u32, h as u32);
            let mut src = crop(&painted, rect);
            let mut at = [rect.x, rect.y];
            let mut levels: Vec<CpuImage> = before.iter().map(as_image).collect();
            for k in 1..MAX_LEVELS {
                let size = [levels[k - 1].width, levels[k - 1].height];
                let half = rect.halved();
                present_rect(&src, at, size, &mut levels[k], half);
                (rect, src, at) = (half, levels[k].clone(), [0, 0]);
            }
            for (k, (got, want)) in levels.iter().zip(&expect).enumerate().skip(1) {
                assert!(got.pixels == want.pixels, "level {k} after {dirty:?}");
            }
        }
    }

    #[test]
    fn aligned_rects_stay_inside_the_level() {
        let r = PixelRect { x: 13, y: 7, width: 5, height: 100 }.aligned(20, 50);
        assert_eq!(r, PixelRect { x: 8, y: 0, width: 12, height: 50 });
        assert_eq!(r.halved(), PixelRect { x: 4, y: 0, width: 6, height: 25 });
    }

    /// A Display P3 pixel is shown as the sRGB color it is: sRGB
    /// (219, 61, 77) is (202, 74, 81) in Display P3. Turned off, the stored
    /// values are shown as they are.
    #[test]
    fn wide_gamut_pixels_show_in_srgb() {
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let p3 = io::IccProfile::from_bytes(p3).unwrap();
        let pixel = [202u16, 74, 81].map(|v| v * 257);
        let shown = |profile| present_rgb16(&pixel, 1, 1, &conversion(profile, display_space()));
        let rgb = |image: PresentedImage| {
            let [b, g, r, _] = image.levels[0].assemble().pixels[..4] else { unreachable!() };
            vec![r, g, b]
        };
        assert_eq!(rgb(shown(None)), [202, 74, 81]);
        let converted = rgb(shown(Some(&p3)));
        for (got, want) in converted.iter().zip([219u8, 61, 77]) {
            assert!(got.abs_diff(want) <= 1, "{converted:?}");
        }
        set_color_managed(false);
        let unmanaged = rgb(shown(Some(&p3)));
        set_color_managed(true);
        assert_eq!(unmanaged, [202, 74, 81]);
    }

    /// On a Display P3 screen a P3 pixel is shown as it is and an sRGB one
    /// as its P3 values, each image saying it is in P3.
    #[test]
    fn a_display_p3_screen_keeps_wide_gamut_pixels() {
        let p3 = moxcms::ColorProfile::new_display_p3().encode().unwrap();
        let p3 = io::IccProfile::from_bytes(p3).unwrap();
        let shown = |pixel: [u16; 3], profile| {
            let image = present_rgb16(&pixel, 1, 1, &conversion(profile, OutputSpace::DisplayP3));
            let [b, g, r, _] = image.levels[0].assemble().pixels[..4] else { unreachable!() };
            (image.space(), [r, g, b])
        };
        let near = |(space, got): (ScreenSpace, [u8; 3]), want: [u8; 3]| {
            assert_eq!(space, ScreenSpace::DisplayP3);
            assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 1), "{got:?}");
        };
        near(shown([202u16, 74, 81].map(|v| v * 257), Some(&p3)), [202, 74, 81]);
        // Pure P3 red, outside sRGB, is not clipped.
        near(shown([65535, 0, 0], Some(&p3)), [255, 0, 0]);
        near(shown([219u16, 61, 77].map(|v| v * 257), None), [202, 74, 81]);
        assert_eq!(present_rgb16(&[0; 3], 1, 1, &ToDisplay::SRGB).space(), ScreenSpace::Srgb);
    }

    #[test]
    fn level_index_skips_levels_the_chain_does_not_have() {
        // Full-size chain: 1/2^k at scale ≤ 1/2^k.
        assert_eq!(level_index(1.0, 0, 4), 0);
        assert_eq!(level_index(0.5, 0, 4), 1);
        assert_eq!(level_index(0.3, 0, 4), 1);
        assert_eq!(level_index(0.01, 0, 4), 3);
        // Half-size chain: level 0 serves everything down to 1/2.
        assert_eq!(level_index(2.0, 1, 3), 0);
        assert_eq!(level_index(0.7, 1, 3), 0);
        assert_eq!(level_index(0.5, 1, 3), 0);
        assert_eq!(level_index(0.25, 1, 3), 1);
        assert_eq!(level_index(0.01, 1, 3), 2);
    }
}
