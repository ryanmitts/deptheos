//! The one input stage (ADR-132): every standard-image decoder hands over
//! a [`Decoded`], and [`input_frame`] turns it into a [`Frame`] in the
//! stack's color space. [`input_header`] sizes it, and refuses what can't
//! be stacked, from the header alone.
//!
//! In order: orientation (applied only here); binary alpha as the data
//! area, partial alpha over white; linear values to sRGB's curve; gray
//! under an RGB profile from its curve; another space converted into the
//! stack's at 16 bits. A frame in the stack's space is never transformed.

use std::path::Path;
use std::sync::Arc;

use image::math::Rect;
use image::metadata::Orientation;
use image::{DynamicImage, ImageBuffer};
use rayon::prelude::*;

use super::load::{Frame, Header};
use super::profile::{IccProfile, SourceSpace, source_space};
use crate::error::{CoreError, Result};
use crate::upload::FrameDepth;

/// How a decoder stores its samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoredDepth {
    U8,
    U16,
    /// 32-bit floating point.
    F32,
}

/// Whether a frame's color samples are multiplied by its alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Straight,
    Premultiplied,
}

/// A decoder's channels, interleaved in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Gray,
    GrayAlpha(Alpha),
    Rgb,
    RgbAlpha(Alpha),
}

impl Layout {
    pub fn channels(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::GrayAlpha(_) => 2,
            Self::Rgb => 3,
            Self::RgbAlpha(_) => 4,
        }
    }

    fn alpha(self) -> Option<Alpha> {
        match self {
            Self::GrayAlpha(a) | Self::RgbAlpha(a) => Some(a),
            Self::Gray | Self::Rgb => None,
        }
    }

    fn is_gray(self) -> bool {
        matches!(self, Self::Gray | Self::GrayAlpha(_))
    }
}

/// What a file says of the color space its values are in, as its decoder
/// found it, ranked by the format's own rules.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorTag {
    /// An embedded ICC profile's bytes.
    Icc(Vec<u8>),
    /// Code points (ITU-T H.273), a PNG's `cICP` or a HEIF's `nclx`.
    Cicp { primaries: u8, transfer: u8, full_range: bool },
    /// A PNG's `sRGB`, `gAMA` and `cHRM` chunks (white, red, green, blue).
    PngChunks { srgb: bool, gamma: Option<f32>, chromaticities: Option<[(f32, f32); 4]> },
    /// An in-camera Adobe RGB JPEG: EXIF's interoperability index `R03`.
    DcfAdobeRgb,
    /// Nothing: sRGB, or linear sRGB for floating-point values.
    Untagged,
}

/// What a decoder says of a frame before decoding its pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedHeader {
    /// As stored, before orientation.
    pub width: u32,
    pub height: u32,
    pub depth: StoredDepth,
    pub layout: Layout,
    pub color: ColorTag,
    /// EXIF's (1–8), or for a HEIF its container's.
    pub orientation: Orientation,
}

/// A decoder's samples, interleaved as its header's [`Layout`] says,
/// rows tightly packed, as stored (not turned).
pub enum Samples {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

/// What every decoder of a standard image hands over.
pub struct Decoded {
    pub header: DecodedHeader,
    pub samples: Samples,
}

/// What a frame is turned into: its own space (a preview, a thumbnail, a
/// file on its own), or a stack's.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Own,
    Stack(&'a StackSpace),
}

/// A stack's color space: its first readable frame's ([`StackSpace::of`]).
/// `None` for sRGB.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackSpace(Option<IccProfile>);

impl StackSpace {
    /// The space a frame with `header` (at `path`) comes out in.
    pub fn of(path: &Path, header: &DecodedHeader) -> Result<Self> {
        Ok(Self(space(path, header)?.encoded))
    }

    /// The space of a stack made already, from its result's `profile`.
    pub fn new(profile: Option<IccProfile>) -> Self {
        Self(profile)
    }

    /// The space's profile; `None` for sRGB.
    pub fn profile(&self) -> Option<&IccProfile> {
        self.0.as_ref()
    }
}

/// A frame that can't be stacked, refused by name from its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// CMYK or YCCK.
    Cmyk,
    /// CIE L*a*b*.
    Lab,
    /// An HDR transfer: PQ or HLG.
    Hdr,
    /// A TIFF layout that isn't read: palette, subsampled YCbCr, more bands
    /// than gray or RGB with one alpha, or samples of another kind.
    TiffLayout,
}

/// What a load changed or couldn't keep, for the user (the app words it).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LoadNote {
    /// Linear values outside 0–1, clipped.
    FloatClipped,
    /// Partial transparency composited over white.
    OverWhite,
    /// Colors clipped converting from `from` into the stack's `into`
    /// (profile names, `None` for an unnamed one).
    Clipped { from: Option<String>, into: Option<String> },
}

/// The upright size and depth [`input_frame`] will make for `into`, or the
/// refusal of a frame that can't be stacked.
pub fn input_header(path: &Path, header: &DecodedHeader, into: Target<'_>) -> Result<Header> {
    let plan = plan(path, header, into)?;
    let (width, height) = upright(header.width, header.height, header.orientation);
    Ok(Header { width, height, depth: plan.depth })
}

/// The decoded frame for `into`, at exactly the size and depth
/// [`input_header`] said.
pub fn input_frame(path: &Path, decoded: Decoded, into: Target<'_>) -> Result<Frame> {
    let Decoded { header, samples } = decoded;
    let plan = plan(path, &header, into)?;
    let (w, h) = (header.width as usize, header.height as usize);
    let channels = header.layout.channels();
    let (len, depth) = match &samples {
        Samples::U8(v) => (v.len(), StoredDepth::U8),
        Samples::U16(v) => (v.len(), StoredDepth::U16),
        Samples::F32(v) => (v.len(), StoredDepth::F32),
    };
    if depth != header.depth || Some(len) != w.checked_mul(h).and_then(|p| p.checked_mul(channels))
    {
        return Err(decode_error(
            path,
            format!(
                "the decoder handed over {len} {depth:?} samples for a {w}×{h} {:?} {:?} header",
                header.layout, header.depth
            ),
        ));
    }
    let alpha = match header.layout.alpha() {
        Some(kind) => classify_alpha(&samples, channels, w, h, kind)
            .ok_or_else(|| decode_error(path, "every pixel is transparent".into()))?,
        None => AlphaUse::Opaque,
    };
    let mut notes = Vec::new();
    let as_stored = matches!(plan.work, Work::AsStored)
        && !matches!(alpha, AlphaUse::Partial(_))
        && depth != StoredDepth::F32;
    let mut image = if as_stored {
        as_stored_rgb(samples, header.layout, w as u32, h as u32)
    } else {
        let (image, clipped) = convert(&samples, header.layout, &alpha, &plan, w as u32, h as u32);
        match (clipped, &plan.work) {
            (Clipped::Source, _) => notes.push(LoadNote::FloatClipped),
            (Clipped::Converted, Work::Transform { from, into, .. }) => {
                notes.push(LoadNote::Clipped { from: from.clone(), into: into.clone() });
            }
            (_, Work::Transform { from, into, .. }) => tracing::info!(
                "{}: converted from {} into {}",
                path.display(),
                from.as_deref().unwrap_or("an unnamed profile"),
                into.as_deref().unwrap_or("an unnamed profile")
            ),
            _ => {}
        }
        image
    };
    if matches!(alpha, AlphaUse::Partial(_)) {
        notes.push(LoadNote::OverWhite);
    }
    let data_area = match alpha {
        AlphaUse::Binary(rect) => {
            fill_outside(&mut image, rect);
            Some(orient_rect(rect, header.width, header.height, header.orientation))
        }
        _ => None,
    };
    image.apply_orientation(header.orientation);
    let expected = (upright(header.width, header.height, header.orientation), Some(plan.depth));
    let made = ((image.width(), image.height()), depth_of(&image));
    if made != expected {
        return Err(decode_error(path, format!("made {made:?}, its header said {expected:?}")));
    }
    Ok(Frame { image, profile: plan.profile, data_area, notes })
}

fn decode_error(path: &Path, message: String) -> CoreError {
    CoreError::Decode { path: path.to_path_buf(), message }
}

fn depth_of(image: &DynamicImage) -> Option<FrameDepth> {
    match image {
        DynamicImage::ImageRgb8(_) => Some(FrameDepth::Eight),
        DynamicImage::ImageRgb16(_) => Some(FrameDepth::Sixteen),
        _ => None,
    }
}

/// `(width, height)` stored, turned by `orientation`.
fn upright(width: u32, height: u32, orientation: Orientation) -> (u32, u32) {
    use Orientation::*;
    match orientation {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (height, width),
        _ => (width, height),
    }
}

/// The space of the frame with `header`, read from its tag.
fn space(path: &Path, header: &DecodedHeader) -> Result<SourceSpace> {
    let float = header.depth == StoredDepth::F32;
    source_space(&header.color, header.layout.is_gray(), float)
        .map_err(|what| CoreError::Refused { path: path.to_path_buf(), what })
}

/// What [`input_frame`] does to a frame's values, from its header.
struct Plan {
    work: Work,
    /// What it comes out at: 8 bits only for 8-bit values kept as stored.
    depth: FrameDepth,
    /// The space it comes out in; `None` for sRGB.
    profile: Option<IccProfile>,
}

enum Work {
    /// In the target's space already: values kept.
    AsStored,
    /// Linear, in the target's primaries: encoded to sRGB's curve.
    Encode,
    /// From another space into the target's (names for a note).
    Transform {
        executor: Arc<moxcms::TransformF32Executor>,
        from: Option<String>,
        into: Option<String>,
    },
}

fn plan(path: &Path, header: &DecodedHeader, into: Target<'_>) -> Result<Plan> {
    let space = space(path, header)?;
    let target = match into {
        Target::Own => space.encoded.clone(),
        Target::Stack(stack) => stack.0.clone(),
    };
    let same = match (&space.encoded, &target) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same_space(b),
        _ => false,
    };
    let work = match (same, space.linear) {
        (true, false) => Work::AsStored,
        (true, true) => Work::Encode,
        (false, _) => {
            let into_profile = match &target {
                Some(profile) => profile.parsed(),
                None => Some(moxcms::ColorProfile::new_srgb()),
            };
            let options = moxcms::TransformOptions {
                rendering_intent: moxcms::RenderingIntent::RelativeColorimetric,
                prefer_fixed_point: false,
                allow_extended_range_rgb_xyz: true,
                ..Default::default()
            };
            let layout = moxcms::Layout::Rgb;
            let executor = into_profile
                .and_then(|p| space.source.create_transform_f32(layout, &p, layout, options).ok())
                .ok_or_else(|| decode_error(path, "its color space can't be converted".into()))?;
            let name = |p: &Option<IccProfile>| match p {
                Some(p) => p.name(),
                None => Some("sRGB".into()),
            };
            Work::Transform { executor, from: name(&space.encoded), into: name(&target) }
        }
    };
    let depth = match (&work, header.depth) {
        (Work::AsStored, StoredDepth::U8) => FrameDepth::Eight,
        _ => FrameDepth::Sixteen,
    };
    Ok(Plan { work, depth, profile: target })
}

/// What a frame's alpha says.
enum AlphaUse {
    /// Every pixel opaque, or no alpha.
    Opaque,
    /// Only on or off (but for a band next to transparent pixels): the
    /// largest rectangle of opaque pixels, as stored.
    Binary(Rect),
    /// Anything else: composited over white.
    Partial(Alpha),
}

/// How wide a band of partial alpha next to transparent pixels a binary
/// alpha may have: a warp's antialiased edge.
const EDGE_BAND: usize = 2;

/// What the alpha channel (the last of `channels`) of `samples` says, or
/// `None` when every pixel is transparent.
fn classify_alpha(
    samples: &Samples,
    channels: usize,
    w: usize,
    h: usize,
    kind: Alpha,
) -> Option<AlphaUse> {
    // 0 transparent, 1 partial, 2 opaque.
    let level = |a: f32| {
        if a <= 0.0 {
            0u8
        } else if a >= 1.0 {
            2
        } else {
            1
        }
    };
    let levels: Vec<u8> = match samples {
        Samples::U8(v) => {
            v.par_chunks(channels).map(|p| level(f32::from(p[channels - 1]) / 255.0)).collect()
        }
        Samples::U16(v) => {
            v.par_chunks(channels).map(|p| level(f32::from(p[channels - 1]) / 65535.0)).collect()
        }
        Samples::F32(v) => v.par_chunks(channels).map(|p| level(p[channels - 1])).collect(),
    };
    let transparent = levels.par_iter().any(|&l| l == 0);
    let partial = levels.par_iter().any(|&l| l == 1);
    if !transparent && !partial {
        return Some(AlphaUse::Opaque);
    }
    // Every partial pixel within the band of a transparent one.
    let near_transparent = |i: usize| {
        let (x, y) = (i % w, i / w);
        let rows = y.saturating_sub(EDGE_BAND)..(y + EDGE_BAND + 1).min(h);
        let cols = x.saturating_sub(EDGE_BAND)..(x + EDGE_BAND + 1).min(w);
        rows.into_iter().any(|r| levels[r * w + cols.start..r * w + cols.end].contains(&0))
    };
    let binary =
        transparent && levels.par_iter().enumerate().all(|(i, &l)| l != 1 || near_transparent(i));
    if !binary {
        return Some(AlphaUse::Partial(kind));
    }
    largest_opaque_rect(&levels, w, h).map(AlphaUse::Binary)
}

/// The largest axis-aligned rectangle of opaque pixels (level 2), by the
/// histogram of each row's opaque run above it; `None` with none.
fn largest_opaque_rect(levels: &[u8], w: usize, h: usize) -> Option<Rect> {
    let mut heights = vec![0usize; w];
    let mut best: Option<(usize, Rect)> = None;
    let mut stack: Vec<usize> = Vec::with_capacity(w + 1);
    for y in 0..h {
        for (x, height) in heights.iter_mut().enumerate() {
            *height = if levels[y * w + x] == 2 { *height + 1 } else { 0 };
        }
        stack.clear();
        for x in 0..=w {
            let current = if x < w { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] < current {
                    break;
                }
                stack.pop();
                let height = heights[top];
                let left = stack.last().map_or(0, |&l| l + 1);
                let area = height * (x - left);
                if area > 0 && best.is_none_or(|(b, _)| area > b) {
                    let rect = Rect {
                        x: left as u32,
                        y: (y + 1 - height) as u32,
                        width: (x - left) as u32,
                        height: height as u32,
                    };
                    best = Some((area, rect));
                }
            }
            stack.push(x);
        }
    }
    best.map(|(_, rect)| rect)
}

/// `rect` in a `w`×`h` frame stored as `orientation` says, upright.
fn orient_rect(rect: Rect, w: u32, h: u32, orientation: Orientation) -> Rect {
    use Orientation::*;
    // Where stored pixel (x, y) lands upright.
    let turn = |x: u32, y: u32| match orientation {
        NoTransforms => (x, y),
        FlipHorizontal => (w - 1 - x, y),
        FlipVertical => (x, h - 1 - y),
        Rotate180 => (w - 1 - x, h - 1 - y),
        Rotate90 => (h - 1 - y, x),
        Rotate270 => (y, w - 1 - x),
        Rotate90FlipH => (y, x),
        Rotate270FlipH => (h - 1 - y, w - 1 - x),
    };
    let a = turn(rect.x, rect.y);
    let b = turn(rect.x + rect.width - 1, rect.y + rect.height - 1);
    let (x, y) = (a.0.min(b.0), a.1.min(b.1));
    Rect { x, y, width: a.0.max(b.0) - x + 1, height: a.1.max(b.1) - y + 1 }
}

/// The pixels of an RGB image outside `rect` filled from its nearest edge,
/// so neither the aligner nor the stackers see a step where its data ends.
fn fill_outside(image: &mut DynamicImage, rect: Rect) {
    fn fill<T: Copy + Send + Sync>(samples: &mut [T], w: usize, rect: Rect) {
        let (x0, y0) = (rect.x as usize, rect.y as usize);
        let (x1, y1) = (x0 + rect.width as usize, y0 + rect.height as usize);
        let row = w * 3;
        samples.par_chunks_mut(row).skip(y0).take(y1 - y0).for_each(|r| {
            let (left, right) = ([r[x0 * 3], r[x0 * 3 + 1], r[x0 * 3 + 2]], {
                let i = (x1 - 1) * 3;
                [r[i], r[i + 1], r[i + 2]]
            });
            r[..x0 * 3].chunks_mut(3).for_each(|p| p.copy_from_slice(&left));
            r[x1 * 3..].chunks_mut(3).for_each(|p| p.copy_from_slice(&right));
        });
        let (top, rest) = samples.split_at_mut(y0 * row);
        let first = rest[..row].to_vec();
        top.chunks_mut(row).for_each(|r| r.copy_from_slice(&first));
        let (inside, bottom) = samples.split_at_mut(y1 * row);
        let last = &inside[(y1 - 1) * row..];
        bottom.chunks_mut(row).for_each(|r| r.copy_from_slice(last));
    }
    let w = image.width() as usize;
    match image {
        DynamicImage::ImageRgb8(i) => fill(i, w, rect),
        DynamicImage::ImageRgb16(i) => fill(i, w, rect),
        _ => unreachable!("the stage makes RGB"),
    }
}

/// Samples kept as RGB at their depth: gray expanded, alpha dropped in
/// place, RGB moved uncopied.
fn as_stored_rgb(samples: Samples, layout: Layout, w: u32, h: u32) -> DynamicImage {
    fn rgb<T: Copy + Send + Sync>(mut samples: Vec<T>, layout: Layout) -> Vec<T> {
        let expanded = |per: usize, samples: &[T]| {
            let mut rgb = Vec::with_capacity(samples.len() / per * 3);
            rgb.extend(samples.chunks(per).flat_map(|p| [p[0]; 3]));
            rgb
        };
        match layout {
            Layout::Rgb => samples,
            Layout::Gray => expanded(1, &samples),
            Layout::GrayAlpha(_) => expanded(2, &samples),
            Layout::RgbAlpha(_) => {
                // In place, front to back: no second frame-sized buffer.
                let pixels = samples.len() / 4;
                for i in 0..pixels {
                    samples.copy_within(4 * i..4 * i + 3, 3 * i);
                }
                samples.truncate(3 * pixels);
                samples.shrink_to_fit();
                samples
            }
        }
    }
    let buffer = |v| ImageBuffer::from_raw(w, h, v).expect("checked against the header");
    match samples {
        Samples::U8(v) => DynamicImage::ImageRgb8(buffer(rgb(v, layout))),
        Samples::U16(v) => DynamicImage::ImageRgb16(
            ImageBuffer::from_raw(w, h, rgb(v, layout)).expect("checked against the header"),
        ),
        Samples::F32(_) => unreachable!("floating point is always quantized"),
    }
}

/// Where values were clipped as a frame was quantized.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Clipped {
    No,
    /// By the conversion into another space.
    Converted,
    /// Already outside 0–1 as decoded (floating point).
    Source,
}

/// Pixels per conversion chunk, so a frame never has a whole float copy.
const CHUNK: usize = 1 << 14;

/// `samples` through `plan`'s work in floating point (partial alpha over
/// white first), quantized to its depth, with where values clipped.
fn convert(
    samples: &Samples,
    layout: Layout,
    alpha: &AlphaUse,
    plan: &Plan,
    w: u32,
    h: u32,
) -> (DynamicImage, Clipped) {
    let channels = layout.channels();
    let gray = layout.is_gray();
    let over_white = match alpha {
        AlphaUse::Partial(kind) => Some(*kind),
        _ => None,
    };
    let pixels = w as usize * h as usize;
    // One chunk's pixels in, as RGB floats: where it clipped as decoded.
    let read = |start: usize, rgb: &mut [f32]| -> bool {
        let mut outside = false;
        for (i, out) in rgb.chunks_mut(3).enumerate() {
            let at = (start + i) * channels;
            let value = |c: usize| match samples {
                Samples::U8(v) => f32::from(v[at + c]) / 255.0,
                Samples::U16(v) => f32::from(v[at + c]) / 65535.0,
                Samples::F32(v) => v[at + c],
            };
            let color = if gray { [value(0); 3] } else { [value(0), value(1), value(2)] };
            outside |= color.iter().any(|v| !(0.0..=1.0).contains(v));
            let over = over_white.map(|kind| (kind, value(channels - 1).clamp(0.0, 1.0)));
            for (o, c) in out.iter_mut().zip(color) {
                *o = match over {
                    Some((Alpha::Straight, a)) => a * c + (1.0 - a),
                    Some((Alpha::Premultiplied, a)) => c + (1.0 - a),
                    None => c,
                };
            }
        }
        outside
    };
    let work = |rgb: &mut Vec<f32>, scratch: &mut Vec<f32>| match &plan.work {
        Work::AsStored => {}
        Work::Encode => rgb.iter_mut().for_each(|v| *v = srgb_encode(*v)),
        Work::Transform { executor, .. } => {
            scratch.resize(rgb.len(), 0.0);
            // The layouts and lengths are the transform's own.
            executor.transform(rgb, scratch).expect("RGB in, RGB out, the same length");
            std::mem::swap(rgb, scratch);
        }
    };
    fn run<T: Copy + Send + Sync + Default>(
        pixels: usize,
        max: f32,
        to: impl Fn(f32) -> T + Sync,
        read: &(dyn Fn(usize, &mut [f32]) -> bool + Sync),
        work: &(dyn Fn(&mut Vec<f32>, &mut Vec<f32>) + Sync),
    ) -> (Vec<T>, Clipped) {
        let mut out = vec![T::default(); pixels * 3];
        let clipped = out
            .par_chunks_mut(CHUNK * 3)
            .enumerate()
            .map(|(chunk, out)| {
                let mut rgb = vec![0.0f32; out.len()];
                let mut scratch = Vec::new();
                let outside = read(chunk * CHUNK, &mut rgb);
                work(&mut rgb, &mut scratch);
                // Clipped where the clamp changes the quantized value.
                let (low, high) = (-0.5 / max, 1.0 + 0.5 / max);
                let mut clipped = false;
                for (o, v) in out.iter_mut().zip(&rgb) {
                    clipped |= *v < low || *v > high;
                    *o = to((v.clamp(0.0, 1.0) * max).round());
                }
                match (outside, clipped) {
                    (true, _) => Clipped::Source,
                    (false, true) => Clipped::Converted,
                    (false, false) => Clipped::No,
                }
            })
            .max()
            .unwrap_or(Clipped::No);
        (out, clipped)
    }
    match plan.depth {
        FrameDepth::Eight => {
            let (v, clipped) = run(pixels, 255.0, |v| v as u8, &read, &work);
            (DynamicImage::ImageRgb8(ImageBuffer::from_raw(w, h, v).expect("sized")), clipped)
        }
        FrameDepth::Sixteen => {
            let (v, clipped) = run(pixels, 65535.0, |v| v as u16, &read, &work);
            (DynamicImage::ImageRgb16(ImageBuffer::from_raw(w, h, v).expect("sized")), clipped)
        }
    }
}

/// IEC 61966-2-1's sRGB curve, encoding linear light, extended: without
/// clamping, and mirrored below zero.
fn srgb_encode(v: f32) -> f32 {
    let a = v.abs();
    let e = if a <= 0.003_130_8 { 12.92 * a } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
    e.copysign(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "fixture";

    fn header(
        w: u32,
        h: u32,
        depth: StoredDepth,
        layout: Layout,
        color: ColorTag,
    ) -> DecodedHeader {
        DecodedHeader {
            width: w,
            height: h,
            depth,
            layout,
            color,
            orientation: Orientation::NoTransforms,
        }
    }

    fn icc(profile: moxcms::ColorProfile) -> ColorTag {
        ColorTag::Icc(profile.encode().unwrap())
    }

    fn stack_of(color: ColorTag) -> StackSpace {
        let h = header(1, 1, StoredDepth::U8, Layout::Rgb, color);
        StackSpace::of(Path::new(PATH), &h).unwrap()
    }

    /// Run the stage on `decoded` for `into`, checking that it makes the
    /// size and depth its header said.
    fn run(decoded: Decoded, into: Target<'_>) -> Frame {
        let path = Path::new(PATH);
        let said = input_header(path, &decoded.header, into).unwrap();
        let frame = input_frame(path, decoded, into).unwrap();
        let depth = match frame.image {
            DynamicImage::ImageRgb8(_) => FrameDepth::Eight,
            _ => FrameDepth::Sixteen,
        };
        assert_eq!(
            (frame.image.width(), frame.image.height(), depth),
            (said.width, said.height, said.depth)
        );
        frame
    }

    fn rgb16(frame: &Frame) -> Vec<u16> {
        frame.image.as_rgb16().expect("16-bit RGB").as_raw().clone()
    }

    /// An opaque 8-bit sRGB frame is handed over as it was, uncopied.
    #[test]
    fn an_srgb_frame_is_kept_as_stored() {
        let pixels: Vec<u8> = (0..4 * 3 * 3).map(|i| i as u8 * 7).collect();
        let decoded = Decoded {
            header: header(4, 3, StoredDepth::U8, Layout::Rgb, ColorTag::Untagged),
            samples: Samples::U8(pixels.clone()),
        };
        let frame = run(decoded, Target::Stack(&StackSpace::default()));
        assert_eq!(frame.image.as_bytes(), pixels.as_slice());
        assert_eq!((frame.profile, frame.data_area, frame.notes), (None, None, vec![]));
    }

    /// Binary alpha (a band of up to 2 px allowed) gives the largest opaque
    /// rectangle, upright, as the data area, filled outside from its edge.
    #[test]
    fn binary_alpha_is_the_frames_data_area() {
        let (w, h) = (10u32, 6u32);
        // Transparent in the left 3 columns, then a 2-px band, then opaque.
        let alpha = |x: u32| match x {
            0..=2 => 0,
            3 | 4 => 128,
            _ => 255,
        };
        let pixels = |alpha: &dyn Fn(u32) -> u8| -> Vec<u8> {
            (0..w * h)
                .flat_map(|i| [(i % w * 20) as u8, (i / w * 40) as u8, 7, alpha(i % w)])
                .collect()
        };
        let decoded = |pixels: Vec<u8>, orientation| Decoded {
            header: DecodedHeader {
                orientation,
                ..header(
                    w,
                    h,
                    StoredDepth::U8,
                    Layout::RgbAlpha(Alpha::Straight),
                    ColorTag::Untagged,
                )
            },
            samples: Samples::U8(pixels),
        };
        let frame = run(decoded(pixels(&alpha), Orientation::NoTransforms), Target::Own);
        assert_eq!(frame.data_area, Some(Rect { x: 5, y: 0, width: 5, height: 6 }));
        assert!(frame.notes.is_empty());
        let rgb = frame.image.as_rgb8().unwrap();
        assert_eq!(rgb.get_pixel(7, 2).0, [140, 80, 7], "inside: as stored");
        assert_eq!(rgb.get_pixel(0, 2).0, [100, 80, 7], "outside: the edge's");
        // Turned a quarter clockwise, the data is in the bottom rows.
        let frame = run(decoded(pixels(&alpha), Orientation::Rotate90), Target::Own);
        assert_eq!(frame.data_area, Some(Rect { x: 0, y: 5, width: 6, height: 5 }));
        // A 3-px band is no antialiased edge.
        let wide = |x: u32| match x {
            0..=1 => 0,
            2..=4 => 128,
            _ => 255,
        };
        let frame = run(decoded(pixels(&wide), Orientation::NoTransforms), Target::Own);
        assert_eq!((frame.data_area, frame.notes), (None, vec![LoadNote::OverWhite]));
    }

    /// Partial alpha is composited over white in the encoded values:
    /// `a·c + (1 − a)` straight, `c + (1 − a)` premultiplied, at the frame's
    /// depth, and noted.
    #[test]
    fn partial_alpha_is_composited_over_white() {
        for (alpha, color, want) in
            [(Alpha::Straight, 1000u16, 33267u16), (Alpha::Premultiplied, 1000, 33767)]
        {
            let decoded = Decoded {
                header: header(
                    1,
                    1,
                    StoredDepth::U16,
                    Layout::GrayAlpha(alpha),
                    ColorTag::Untagged,
                ),
                samples: Samples::U16(vec![color, 32768]),
            };
            let frame = run(decoded, Target::Own);
            assert_eq!(rgb16(&frame), [want; 3], "{alpha:?}");
            assert_eq!(frame.notes, [LoadNote::OverWhite]);
        }
    }

    /// Linear values are encoded to sRGB's curve at 16 bits, unclamped
    /// until quantized, where what is above 1 clips once, noted.
    #[test]
    fn linear_values_are_encoded_to_the_srgb_curve() {
        let float = |values: Vec<f32>| Decoded {
            header: header(2, 1, StoredDepth::F32, Layout::Rgb, ColorTag::Untagged),
            samples: Samples::F32(values),
        };
        let frame = run(float(vec![0.0, 0.18, 1.0, 0.002, 0.5, 0.75]), Target::Own);
        assert_eq!(frame.profile, None, "linear sRGB comes out sRGB");
        assert_eq!(rgb16(&frame), [0, 30235, 65535, 1693, 48192, 57725]);
        assert!(frame.notes.is_empty());
        let frame = run(float(vec![2.0, 0.5, -0.1, 0.0, 0.0, 0.0]), Target::Own);
        assert_eq!(frame.notes, [LoadNote::FloatClipped]);
        assert_eq!(&rgb16(&frame)[..3], [65535, 48192, 0]);
        // An 8-bit PNG with gAMA 1.0 comes out 16-bit, its shadows apart.
        let png = ColorTag::PngChunks { srgb: false, gamma: Some(1.0), chromaticities: None };
        let decoded = Decoded {
            header: header(1, 1, StoredDepth::U8, Layout::Gray, png),
            samples: Samples::U8(vec![1]),
        };
        assert_eq!(rgb16(&run(decoded, Target::Own)), [3266; 3]);
    }

    /// A gray frame's space is RGB made from its curve: an all-gray stack
    /// keeps its values (at their depth, under that profile); a gray frame
    /// in an sRGB stack is converted; untagged gray in sRGB is sRGB.
    #[test]
    fn gray_frames_keep_their_values_among_gray() {
        let gray22 = icc(moxcms::ColorProfile::new_gray_with_gamma(2.2));
        let decoded = |color: ColorTag| Decoded {
            header: header(2, 1, StoredDepth::U8, Layout::Gray, color),
            samples: Samples::U8(vec![60, 200]),
        };
        let stack = stack_of(ColorTag::Icc(match &gray22 {
            ColorTag::Icc(b) => b.clone(),
            _ => unreachable!(),
        }));
        let h = header(1, 1, StoredDepth::U8, Layout::Gray, gray22.clone());
        let stack = StackSpace::of(Path::new(PATH), &h).unwrap_or(stack);
        assert!(stack.profile().is_some(), "gamma 2.2 is not sRGB's curve");
        let frame = run(decoded(gray22.clone()), Target::Stack(&stack));
        assert_eq!(frame.image.as_bytes(), [60, 60, 60, 200, 200, 200]);
        assert_eq!(frame.profile.as_ref(), stack.profile());
        let frame = run(decoded(gray22), Target::Stack(&StackSpace::default()));
        let converted = rgb16(&frame);
        assert!(converted[0] == converted[1] && converted[1] == converted[2], "gray stays gray");
        assert_ne!(converted[0], 60 * 257, "converted to sRGB's curve");
        let frame = run(decoded(ColorTag::Untagged), Target::Stack(&StackSpace::default()));
        assert_eq!(frame.image.as_bytes(), [60, 60, 60, 200, 200, 200]);
    }

    /// A frame in another space than the stack's is converted into it at
    /// 16 bits, its colors outside it noted; one in the same space, under
    /// another name, is never transformed.
    #[test]
    fn other_spaces_are_converted_into_the_stacks() {
        let p3 = icc(moxcms::ColorProfile::new_display_p3());
        let red = |color: ColorTag| Decoded {
            header: header(1, 1, StoredDepth::U8, Layout::Rgb, color),
            samples: Samples::U8(vec![255, 0, 0]),
        };
        let srgb = StackSpace::default();
        let frame = run(red(p3.clone()), Target::Stack(&srgb));
        assert_eq!(rgb16(&frame), [65535, 0, 0], "P3's red is outside sRGB");
        assert_eq!(
            frame.notes,
            [LoadNote::Clipped { from: Some("Display P3".into()), into: Some("sRGB".into()) }]
        );
        let p3_stack = stack_of(p3.clone());
        let frame = run(red(ColorTag::Untagged), Target::Stack(&p3_stack));
        assert!(frame.notes.is_empty(), "sRGB fits in P3");
        let [r, g, b] = <[u16; 3]>::try_from(rgb16(&frame)).unwrap();
        assert!(r > 55000 && r < 65535 && g > 10000 && b > 5000, "{r} {g} {b}");
        let mut renamed = moxcms::ColorProfile::new_display_p3();
        renamed.description = Some(moxcms::ProfileText::PlainString("Another P3".into()));
        let frame = run(red(icc(renamed)), Target::Stack(&p3_stack));
        assert_eq!(frame.image.as_bytes(), [255, 0, 0], "the same space under another name");
    }

    /// What can't be stacked is refused from the header, before anything
    /// is decoded; a decoder that hands over other than its header said is
    /// an error, not an assert.
    #[test]
    fn refusals_and_mismatches_are_errors() {
        let pq = ColorTag::Cicp { primaries: 9, transfer: 16, full_range: true };
        let h = header(1, 1, StoredDepth::U16, Layout::Rgb, pq);
        let error = input_header(Path::new(PATH), &h, Target::Own).unwrap_err();
        assert!(matches!(error, CoreError::Refused { what: Refused::Hdr, .. }), "{error}");
        let short = Decoded {
            header: header(2, 2, StoredDepth::U8, Layout::Rgb, ColorTag::Untagged),
            samples: Samples::U8(vec![0; 3]),
        };
        let error = input_frame(Path::new(PATH), short, Target::Own).err().unwrap();
        assert!(matches!(error, CoreError::Decode { .. }), "{error}");
    }

    /// Each input the stage changes on purpose (ADR-132), pinned by a hash
    /// of its output in an sRGB stack.
    #[test]
    fn deliberately_changed_inputs_are_pinned() {
        let (w, h) = (23u32, 17u32);
        let n = (w * h) as usize;
        let ramp = |c: usize| (0..n).map(move |i| ((i * (7 + c * 5)) % 256) as u8);
        let rgba: Vec<u8> = (0..n)
            .flat_map(|i| {
                let a = if i % w as usize > 3 { 255 } else { (i * 37 % 256) as u8 };
                [(i * 7 % 256) as u8, (i * 11 % 256) as u8, (i * 13 % 256) as u8, a]
            })
            .collect();
        let binary: Vec<u8> = rgba
            .chunks(4)
            .enumerate()
            .flat_map(|(i, p)| [p[0], p[1], p[2], if i % w as usize > 5 { 255 } else { 0 }])
            .collect();
        let rgb: Vec<u8> =
            ramp(0).zip(ramp(1)).zip(ramp(2)).flat_map(|((r, g), b)| [r, g, b]).collect();
        let floats: Vec<f32> = rgb.iter().map(|&v| f32::from(v) / 200.0).collect();
        let decoded = |depth, layout, color, samples| Decoded {
            header: header(w, h, depth, layout, color),
            samples,
        };
        let linear_png =
            ColorTag::PngChunks { srgb: false, gamma: Some(1.0), chromaticities: None };
        let cases = [
            decoded(
                StoredDepth::U8,
                Layout::RgbAlpha(Alpha::Straight),
                ColorTag::Untagged,
                Samples::U8(rgba.clone()),
            ),
            decoded(
                StoredDepth::U8,
                Layout::RgbAlpha(Alpha::Premultiplied),
                ColorTag::Untagged,
                Samples::U8(rgba),
            ),
            decoded(
                StoredDepth::U8,
                Layout::RgbAlpha(Alpha::Straight),
                ColorTag::Untagged,
                Samples::U8(binary),
            ),
            decoded(StoredDepth::F32, Layout::Rgb, ColorTag::Untagged, Samples::F32(floats)),
            decoded(StoredDepth::U8, Layout::Rgb, linear_png, Samples::U8(rgb.clone())),
            decoded(
                StoredDepth::U8,
                Layout::Gray,
                icc(moxcms::ColorProfile::new_gray_with_gamma(1.8)),
                Samples::U8(ramp(0).collect()),
            ),
            decoded(
                StoredDepth::U8,
                Layout::Rgb,
                icc(moxcms::ColorProfile::new_display_p3()),
                Samples::U8(rgb.clone()),
            ),
            decoded(StoredDepth::U8, Layout::Rgb, ColorTag::DcfAdobeRgb, Samples::U8(rgb)),
        ];
        let got: Vec<u64> = cases
            .into_iter()
            .map(|d| {
                let frame = run(d, Target::Stack(&StackSpace::default()));
                frame.image.as_bytes().iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
                    (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
                })
            })
            .collect();
        let pinned = [
            0xf043_ffc3_18a9_511b,
            0x0ffd_b2e2_cc71_76de,
            0x67d2_2dc8_a74a_1d31,
            0x5eec_e8d8_c5f0_2641,
            0x708f_b2d2_d278_cbb4,
            0x1fff_3c79_7328_2bed,
            0x988c_1716_49af_2d24,
            0xd81a_43a0_764a_5cac,
        ];
        // On x86-64 moxcms picks its SIMD path by the CPU at run time, so the
        // Adobe RGB conversion's last bits vary between machines; it is pinned on ARM64.
        let n = if cfg!(target_arch = "aarch64") { pinned.len() } else { pinned.len() - 1 };
        assert_eq!(got[..n], pinned[..n], "{got:#x?}");
    }
}
