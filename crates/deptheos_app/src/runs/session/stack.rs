//! What a run makes: a [`CompletedStack`] of CPU data, with its geometry
//! ([`StackGeometry`]), color model ([`StackColor`]) and saving
//! ([`SaveOptions`]).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use deptheos_core::{Affine2, FrameAlignment, PlaneRamp, io};
use deptheos_render::Draw;
use deptheos_render::dng_render::Constants;
use image::math::Rect;
use image::{ImageBuffer, Luma};

use super::SessionOptions;
use super::readback::download_rgb;
use crate::engine::look::Look;
use crate::engine::present::conversion;
use crate::engine::present::{PresentedImage, present_frame_map};
use crate::frames::names::file_label;
use crate::runs::stacker::StackerConfig;
use crate::words::{Words, say};
use crate::{Engine, Error, Result};

/// How [`CompletedStack::save`] encodes PNG, TIFF and JPEG (DNG is always
/// the linear pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveOptions {
    /// JPEG quality, 1–100.
    pub jpeg_quality: u8,
    /// PNG and TIFF keep 16 bits where the result has them; off rounds to 8.
    pub sixteen_bit: bool,
    /// How TIFF files compress their pixels.
    pub tiff_compression: io::TiffCompression,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            jpeg_quality: io::JPEG_QUALITY,
            sixteen_bit: true,
            tiff_compression: io::TiffCompression::default(),
        }
    }
}

impl SaveOptions {
    /// The core's encoding settings.
    pub fn encoding(&self) -> io::Encoding {
        io::Encoding { jpeg_quality: self.jpeg_quality, tiff_compression: self.tiff_compression }
    }
}

/// Where each frame landed in a finished stack: per-frame transforms and the
/// crop. Used to compare sources and to replay the alignment
/// ([`super::StackingSession::with_alignment`]).
#[derive(Clone, Debug, PartialEq)]
pub struct StackGeometry {
    /// Per-frame transform and brightness factor, one per path **in the
    /// order given**; `None` when alignment was off.
    pub transforms: Vec<Option<FrameAlignment>>,
    /// The result's crop, in full-frame coordinates, when it trimmed any.
    pub crop: Option<Rect>,
    /// Stacked last to first, so the last path is the reference.
    pub reversed: bool,
    /// Frames left out, by index into the paths, in order; each keeps its
    /// place (and plane), and its transform is `None`.
    pub left_out: Vec<LeftOut>,
}

/// A frame a stack left out (unreadable or unalignable), and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftOut {
    pub index: usize,
    pub reason: String,
}

/// `index` of `n` frames mapped between given and processing order; the
/// same unless `reversed`.
pub(super) fn flip(reversed: bool, n: usize, index: usize) -> usize {
    if reversed { n - 1 - index } else { index }
}

impl StackGeometry {
    /// Whether frame `index` was left out.
    pub fn is_left_out(&self, index: usize) -> bool {
        self.left_out.iter().any(|l| l.index == index)
    }

    /// Frame `index` between path and accumulation order.
    pub(crate) fn accumulated(&self, index: usize) -> usize {
        flip(self.reversed, self.transforms.len(), index)
    }

    /// The frames stacked (indexes into the paths), in stacking order.
    pub(crate) fn stacked_order(&self) -> Vec<usize> {
        let n = self.transforms.len();
        (0..n).map(|i| self.accumulated(i)).filter(|&i| !self.is_left_out(i)).collect()
    }

    /// The reference: the first stacked, whose metadata exports carry.
    pub fn reference(&self) -> Option<usize> {
        self.stacked_order().first().copied()
    }
}

/// What a stack left out, for a notice: "Left out a.CR3 (why)"; `None`
/// when nothing was. Reasons are in the language they were made in.
pub fn left_out_message<'a>(
    words: &Words,
    frames: impl IntoIterator<Item = (&'a Path, &'a str)>,
) -> Option<String> {
    let frames: Vec<String> = frames
        .into_iter()
        .map(|(path, reason)| {
            say!(words, "left-out-item", name = file_label(path), reason = reason)
        })
        .collect();
    match frames.len() {
        0 => None,
        1 => Some(say!(words, "left-out-frame", frame = frames[0].as_str())),
        n => {
            let frames = frames
                .into_iter()
                .reduce(|items, next| say!(words, "left-out-items", items = items, next = next));
            Some(say!(words, "left-out-frames", count = n, frames = frames.unwrap_or_default()))
        }
    }
}

/// "2 frames left out", before the button that selects them.
pub fn left_out_line(words: &Words, count: usize) -> String {
    say!(words, "left-out-line", count = count)
}

impl CompletedStack {
    /// Whether this stack stacked `path` (included it and did not leave it out).
    pub fn stacked(&self, path: &Path) -> bool {
        self.paths.iter().position(|p| p == path).is_some_and(|i| !self.geometry.is_left_out(i))
    }

    /// The frames left out, with each lower-case reason.
    pub fn left_out_frames(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.geometry
            .left_out
            .iter()
            .filter_map(|l| Some((self.paths.get(l.index)?.as_path(), l.reason.as_str())))
    }

    /// [`left_out_message`] for this stack.
    pub fn left_out_message(&self, words: &Words) -> Option<String> {
        left_out_message(words, self.left_out_frames())
    }
}

/// How many of `frames` may be left out: one in ten, 1 to 3. More likely
/// means frames that aren't one stack.
pub(crate) fn left_out_limit(frames: usize) -> usize {
    (frames / 10).clamp(1, 3)
}

/// The color model a stack ran in, from its frames' kind.
// One per stack, so the size spread between variants is harmless.
#[allow(clippy::large_enum_variant)]
pub enum StackColor {
    /// Standard sources: stored values, sRGB unless `profile` says otherwise.
    Srgb {
        /// Some source frame had 16-bit channels (PNG/TIFF save at 16 bits).
        sixteen_bit: bool,
        /// The reference's color space when not sRGB, embedded in exports.
        profile: Option<io::IccProfile>,
    },
    /// Camera RAW/DNG sources: the pixels are linear camera RGB.
    Linear {
        /// The reference frame's color science, for DNG output.
        metadata: deptheos_dng::DngImage,
        /// How the linear pixels render to sRGB.
        render_params: deptheos_dng::RenderParams,
    },
}

/// How `metadata`'s linear pixels render to sRGB, or sRGB identity (logged)
/// when that can't be computed, so the picture still shows.
pub(crate) fn render_params_or_identity(
    metadata: &deptheos_dng::DngImage,
) -> deptheos_dng::RenderParams {
    metadata.render_params().unwrap_or_else(|e| {
        tracing::warn!("render params failed ({e}), falling back to sRGB identity");
        deptheos_dng::RenderParams::srgb_identity()
    })
}

/// A one-channel 16-bit image: a stack's frame map.
pub type Gray16Image = ImageBuffer<Luma<u16>, Vec<u16>>;

/// A finished stack: everything needed to show, save, compare or persist
/// it, on the CPU.
pub struct CompletedStack {
    pub width: u32,
    pub height: u32,
    /// The cropped result, interleaved RGB u16: linear camera RGB for
    /// [`StackColor::Linear`], else sRGB.
    pub pixels: Vec<u16>,
    /// Which frame each pixel came from, as its plane ([`PlaneRamp::plane`])
    /// × 65535, cropped like the result. `None` for a render saved without.
    pub frame_map: Option<Gray16Image>,
    pub color: StackColor,
    /// Source frames in the order they were given.
    pub paths: Vec<PathBuf>,
    pub geometry: StackGeometry,
    pub config: StackerConfig,
    pub options: SessionOptions,
    /// What loading the frames assumed or lost ([`io::LoadNote`]); empty
    /// for a render read from disk.
    pub notes: Vec<String>,
    /// [`Self::frame_contributions`], once counted.
    pub contributions: super::Contributions,
}

impl CompletedStack {
    /// A bare sRGB stack of `pixels` from `paths`, with defaults elsewhere,
    /// for tests, benches and bindings.
    pub fn srgb(width: u32, height: u32, pixels: Vec<u16>, paths: Vec<PathBuf>) -> Self {
        assert_eq!(pixels.len(), 3 * width as usize * height as usize, "RGB pixels");
        Self {
            width,
            height,
            pixels,
            frame_map: None,
            color: StackColor::Srgb { sixteen_bit: true, profile: None },
            geometry: StackGeometry {
                transforms: vec![None; paths.len()],
                crop: None,
                reversed: false,
                left_out: Vec::new(),
            },
            paths,
            config: StackerConfig::Pyramid(deptheos_core::pyramid::PyramidConfig::default()),
            options: SessionOptions::default(),
            notes: Vec::new(),
            contributions: Default::default(),
        }
    }

    /// The frames left out, then the load notes; `None` when nothing.
    pub fn notice(&self, words: &Words) -> Option<String> {
        let parts: Vec<String> =
            self.left_out_message(words).into_iter().chain(self.notes.clone()).collect();
        (!parts.is_empty()).then(|| crate::words::notices(words, &parts))
    }

    /// Whether the stack is linear camera RGB, as DNG output needs.
    pub fn is_linear(&self) -> bool {
        matches!(self.color, StackColor::Linear { .. })
    }

    /// Whether PNG and TIFF can keep 16 bits: linear stacks and stacks of
    /// 16-bit frames.
    pub fn has_sixteen_bit(&self) -> bool {
        match &self.color {
            StackColor::Srgb { sixteen_bit, .. } => *sixteen_bit,
            StackColor::Linear { .. } => true,
        }
    }

    /// A standard stack's color space when not sRGB; `None` otherwise.
    pub fn profile(&self) -> Option<&io::IccProfile> {
        match &self.color {
            StackColor::Srgb { profile, .. } => profile.as_ref(),
            StackColor::Linear { .. } => None,
        }
    }

    /// How linear pixels render to sRGB; `None` for sRGB stacks.
    pub fn render_params(&self) -> Option<&deptheos_dng::RenderParams> {
        match &self.color {
            StackColor::Linear { render_params, .. } => Some(render_params),
            StackColor::Srgb { .. } => None,
        }
    }

    /// The plane of frame `index` on [`Self::frame_map`], with its blend
    /// ramp; planes follow accumulation order and span left-out frames.
    pub fn frame_plane(&self, index: usize) -> PlaneRamp {
        let g = &self.geometry;
        PlaneRamp::new(g.accumulated(index) as u32, self.paths.len() as u32, |j| {
            !g.is_left_out(g.accumulated(j as usize))
        })
    }

    /// The sRGB legend color of frame `index` on [`Self::present_map`];
    /// `None` for a left-out frame.
    pub fn frame_color(&self, index: usize) -> Option<[u8; 3]> {
        if index >= self.paths.len() || self.geometry.is_left_out(index) {
            return None;
        }
        Some(crate::engine::present::legend_color(
            self.geometry.accumulated(index),
            self.paths.len(),
        ))
    }

    /// The depth map as displayed, each frame in its legend color
    /// ([`Self::frame_color`]). With `lit` (an index into `paths`) the rest
    /// is dimmed; a left-out `lit` shows the map whole. `None` without a
    /// frame map.
    pub fn present_map(&self, lit: Option<usize>) -> Option<PresentedImage> {
        let map = self.frame_map.as_ref()?;
        let ramp = lit
            .filter(|&i| i < self.paths.len() && !self.geometry.is_left_out(i))
            .map(|i| self.frame_plane(i));
        Some(present_frame_map(map, ramp.as_ref()))
    }

    /// The frame (into [`Self::paths`]) with the largest share of the pixel
    /// at `x`, `y`; `None` without a frame map, or outside it.
    pub fn frame_at(&self, x: u32, y: u32) -> Option<usize> {
        let map = self.frame_map.as_ref()?;
        if x >= map.width() || y >= map.height() {
            return None;
        }
        let z = f32::from(map.get_pixel(x, y).0[0]) / 65535.0;
        (0..self.paths.len())
            .filter(|&i| !self.geometry.is_left_out(i))
            .map(|i| (i, self.frame_plane(i).weight(z)))
            .filter(|&(_, w)| w > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// The backward map from this stack's full-frame coordinates to
    /// `other`'s result pixels, for [`deptheos_core::warp`].
    ///
    /// Through a frame `c` both stacked, `T_other(c)⁻¹ · T_self(c)`, then
    /// `other`'s crop origin. `None` when they share no frame recorded the
    /// same way (aligned in both or neither), or decoded RAW with different
    /// backends, whose pixels don't line up.
    ///
    /// With it comes the exposure match: `other`'s pixels times
    /// `brightness` (`b_self(c) / b_other(c)`) read in this stack's exposure.
    pub fn alignment_to(&self, other: &CompletedStack) -> Option<FrameAlignment> {
        if self.options.raw_backend != other.options.raw_backend {
            return None;
        }
        let origin = other.geometry.crop.map_or((0.0, 0.0), |r| (r.x as f32, r.y as f32));
        #[rustfmt::skip]
        let to_pixels = Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
            1.0, 0.0, -origin.0,
            0.0, 1.0, -origin.1,
            0.0, 0.0, 1.0,
        ));
        let through =
            through_shared_frame(&self.paths, &self.geometry, &other.paths, &other.geometry)?;
        Some(FrameAlignment::new(to_pixels * through.transform, through.brightness))
    }
}

impl StackGeometry {
    /// How the frame at `path` lands on this stack's result, in full-frame
    /// coordinates, for [`deptheos_core::warp`]: its own alignment, else a
    /// sibling's (same RAW backend) carried over through a shared frame.
    /// `None` when nothing relates them.
    pub(crate) fn frame_alignment<'a>(
        &self,
        my_paths: &[PathBuf],
        path: &Path,
        siblings: impl IntoIterator<Item = (&'a [PathBuf], &'a StackGeometry)>,
    ) -> Option<FrameAlignment> {
        if let Some(i) = my_paths.iter().position(|p| p == path)
            && !self.is_left_out(i)
        {
            return Some(self.transforms.get(i)?.unwrap_or_else(FrameAlignment::identity));
        }
        siblings.into_iter().find_map(|(paths, geometry)| {
            let j = paths.iter().position(|p| p == path)?;
            if geometry.is_left_out(j) {
                return None;
            }
            let to_sibling = through_shared_frame(my_paths, self, paths, geometry)?;
            let frame = match geometry.transforms.get(j)? {
                Some(frame) => *frame,
                // An unaligned sibling relates only through unaligned frames.
                None => FrameAlignment::identity(),
            };
            Some(FrameAlignment::new(
                frame.transform * to_sibling.transform,
                frame.brightness * to_sibling.brightness,
            ))
        })
    }
}

/// Writes `pixels` (`look`'s stack's own, or retouched) to `path` tagged
/// `tags`, its format from the extension: `.dng` the linear pixels (linear
/// stacks only), others drawn in `look` ([`export_pixels`]).
pub(crate) fn export(
    engine: &Engine,
    look: Look<'_>,
    pixels: &[u16],
    path: &Path,
    save: SaveOptions,
    tags: &io::metadata::ExportMetadata,
) -> Result<()> {
    let stack = look.stack();
    let (width, height) = (stack.width, stack.height);
    let (encoding, allow_sixteen) = (save.encoding(), save.sixteen_bit);
    let sixteen = match &stack.color {
        StackColor::Linear { metadata, .. } if io::is_dng(path) => {
            let dng_tags = deptheos_dng::DngTags {
                software: &tags.software,
                comment: tags.comment.as_deref().unwrap_or_default(),
                xmp: tags.xmp.as_deref().unwrap_or_default(),
                capture_time: &crate::library::export_tags::capture_time_iso(tags),
                fields: &crate::library::export_tags::dng_fields(tags),
            };
            if !look.reframe().is_none() {
                return Err(Error::InvalidState("a DNG keeps the whole picture".into()));
            }
            // Written to a temporary moved into place, as every save.
            return io::write_replacing(path, |tmp| {
                Ok(metadata.write_tagged(tmp, pixels, width, height, 3, &dng_tags)?)
            })
            .map_err(Error::from);
        }
        _ if io::is_dng(path) => return Err(Error::NotRaw),
        StackColor::Linear { .. } => allow_sixteen,
        StackColor::Srgb { sixteen_bit, .. } => *sixteen_bit && allow_sixteen,
    };
    let drawn = export_pixels(engine, look, pixels, (width, height))?;
    let reframe = look.reframe();
    if reframe.is_none() {
        io::save_rgb16(path, &drawn, width, height, sixteen, encoding, tags)?;
    } else {
        let picture = deptheos_render::reframe::Size { width, height };
        let (framed, size) = reframe.apply(&drawn, picture, 3).map_err(|e| {
            Error::InvalidState(format!("{:?} is outside the {width} × {height} picture", e.crop))
        })?;
        io::save_rgb16(path, &framed, size.width, size.height, sixteen, encoding, tags)?;
    }
    Ok(())
}

/// `pixels` (`width`×`height`, of `look`'s stack or a region or snapshot
/// of it) as an export holds them: rendered on the GPU, converted, or as
/// stored.
pub(crate) fn export_pixels<'p>(
    engine: &Engine,
    look: Look<'_>,
    pixels: &'p [u16],
    (width, height): (u32, u32),
) -> Result<Cow<'p, [u16]>> {
    Ok(match look.resolve() {
        Draw::Rendered(constants) => {
            Cow::Owned(render_pixels(engine, pixels, width, height, &constants)?)
        }
        Draw::Stored { profile, output } => match conversion(profile, output) {
            Some(to_srgb) => {
                let mut out = vec![0; pixels.len()];
                to_srgb.convert(pixels, &mut out);
                Cow::Owned(out)
            }
            None => Cow::Borrowed(pixels),
        },
    })
}

/// `pixels` rendered through `constants` on the GPU and read back.
fn render_pixels(
    engine: &Engine,
    pixels: &[u16],
    width: u32,
    height: u32,
    constants: &Constants,
) -> Result<Vec<u16>> {
    let ctx = engine.gpu();
    let input = match constants.linearizer() {
        Some(linearizer) => linearizer.upload(ctx, pixels, width, height),
        None => {
            // A one-off: its frame-sized staging buffer is not kept.
            let mut uploader = engine.common().uploader();
            let input = crate::engine::preview::upload_rgb16_texture(
                ctx,
                &mut uploader,
                pixels,
                width,
                height,
            );
            uploader.release_resources();
            input?
        }
    };
    let rendered = engine.color().render(ctx, &input, constants);
    download_rgb(ctx, &rendered, Rect { x: 0, y: 0, width, height })
}

/// A's reference coordinates to B's and B's exposure to A's, through the
/// first shared frame recorded the same way in both: `T_b(c)⁻¹ · T_a(c)`
/// and `b_a(c) / b_b(c)`.
fn through_shared_frame(
    a_paths: &[PathBuf],
    a: &StackGeometry,
    b_paths: &[PathBuf],
    b: &StackGeometry,
) -> Option<FrameAlignment> {
    a_paths.iter().zip(&a.transforms).enumerate().find_map(|(index, (path, mine))| {
        let theirs = b_paths.iter().position(|p| p == path)?;
        if a.is_left_out(index) || b.is_left_out(theirs) {
            return None;
        }
        match (mine, b.transforms.get(theirs)?) {
            (Some(mine), Some(theirs)) => Some(FrameAlignment::new(
                theirs.transform.try_inverse()? * mine.transform,
                mine.brightness / theirs.brightness,
            )),
            (None, None) => Some(FrameAlignment::identity()),
            _ => None,
        }
    })
}

#[cfg(test)]
mod frame_at_tests {
    use super::*;

    /// A 3×1 stack of four frames whose frame map holds each pixel's plane.
    fn stack(planes: [u16; 3]) -> CompletedStack {
        let paths = (0..4).map(|i| PathBuf::from(format!("f{i}.jpg"))).collect();
        let map = Gray16Image::from_raw(3, 1, planes.to_vec()).unwrap();
        CompletedStack { frame_map: Some(map), ..CompletedStack::srgb(3, 1, vec![0; 9], paths) }
    }

    #[test]
    fn the_frame_whose_plane_is_nearest() {
        // Planes 0, 1/3, 2/3 and 1 for four frames.
        let s = stack([0, 65535 / 3 + 100, 65535]);
        assert_eq!(s.frame_at(0, 0), Some(0));
        assert_eq!(s.frame_at(1, 0), Some(1));
        assert_eq!(s.frame_at(2, 0), Some(3));
        assert_eq!(s.frame_at(3, 0), None);
    }

    #[test]
    fn none_without_a_frame_map() {
        let s = CompletedStack::srgb(1, 1, vec![0; 3], vec![PathBuf::from("a.jpg")]);
        assert_eq!(s.frame_at(0, 0), None);
    }

    #[test]
    fn left_out_frame_uses_sibling_alignment_instead_of_identity() {
        let paths = vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg"), PathBuf::from("c.jpg")];
        let shifted = FrameAlignment::new(
            Affine2::from_matrix_unchecked(nalgebra::Matrix3::new(
                1.0, 0.0, 5.0, 0.0, 1.0, -3.0, 0.0, 0.0, 1.0,
            )),
            1.25,
        );
        let mut s = CompletedStack::srgb(1, 1, vec![0; 3], paths.clone());
        s.geometry.transforms = vec![Some(FrameAlignment::identity()), None, Some(shifted)];
        s.geometry.left_out = vec![LeftOut { index: 1, reason: "blurry".into() }];

        // Without a sibling that stacked `b.jpg`, it has no alignment.
        assert_eq!(
            s.geometry.frame_alignment(&s.paths, Path::new("b.jpg"), std::iter::empty()),
            None
        );

        // A sibling that stacked both gives `b.jpg` its alignment.
        let sibling = StackGeometry {
            transforms: vec![Some(FrameAlignment::identity()), Some(shifted), None],
            crop: None,
            reversed: false,
            left_out: vec![LeftOut { index: 2, reason: "blurry".into() }],
        };
        assert_eq!(
            s.geometry.frame_alignment(
                &s.paths,
                Path::new("b.jpg"),
                [(paths.as_slice(), &sibling)]
            ),
            Some(shifted)
        );
    }
}
